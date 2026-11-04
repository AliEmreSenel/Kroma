"""Opt-in integration tests: real Kroma rendering, lifecycle and nested feedback."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

CLI = os.environ.get('KROMA_TEST_CLI')
DAEMON = os.environ.get('KROMA_TEST_DAEMON')


@unittest.skipUnless(CLI and DAEMON, 'set KROMA_TEST_CLI and KROMA_TEST_DAEMON for GPU integration')
class NativePreviewTests(unittest.TestCase):
    def invoke(self, *args):
        return subprocess.check_output([CLI, *map(str, args)], text=True, env=dict(os.environ, KROMA_DAEMON=DAEMON))

    def rgb(self, path):
        data = subprocess.check_output(['ffmpeg','-nostdin','-v','error','-i',str(path),'-vf','scale=1:1','-pix_fmt','rgb24','-f','rawvideo','pipe:1'])
        return list(zip(data[0::3],data[1::3],data[2::3]))

    def test_shader_lifecycle_and_low_fps_use_rendered_frames_and_active_poster(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'config.toml').write_text("[meta]\nname='Lifecycle preview'\nauthor='test'\n[rendering]\ntarget_fps=5\n[states.load]\nlength=0.7\nshader='red.frag'\n[states.active]\nlength=0\nshader='green.frag'\n")
            for name,color in [('red','1,0,0,1'),('green','0,1,0,1')]:
                (root/f'{name}.frag').write_text('#version 450\nlayout(location=0) out vec4 color;\nvoid main(){color=vec4('+color+');}')
            package = root/'lifecycle.shade'
            self.invoke('pack',root,'--output',package)
            self.invoke('media','render-preview',package,'--embed','--seconds','1.4','--daemon',DAEMON)
            info = json.loads(self.invoke('media','info',package))
            self.assertEqual(info['preview_renderer'],'kroma')
            self.assertEqual(info['kind'],'shader')
            poster,clip = root/'poster.jpg',root/'clip.mp4'
            self.invoke('media','preview',package,'--thumbnail',poster,'--video',clip)
            frames = self.rgb(clip)
            self.assertEqual(len(frames),17)
            self.assertGreater(frames[0][0],180)
            self.assertLess(frames[0][1],50)
            self.assertGreater(frames[-1][1],180)
            self.assertLess(frames[-1][0],50)
            self.assertGreater(self.rgb(poster)[0][1],180)

    def test_nested_feedback_advances_in_actual_kroma_renderer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'config.toml').write_text("[meta]\nname='Nested feedback'\nauthor='test'\n[states.active]\nlength=0\n[states.active.textures.scene]\ntype='shader'\nshader='feedback.frag'\nwidth=1920\nheight=1080\n[states.active.textures.scene.textures.previous]\ntype='shader'\ninput='t-1'\n")
            (root/'feedback.frag').write_text('#version 450\nlayout(set=1,binding=0) uniform texture2D previous;\nlayout(set=1,binding=1) uniform sampler previous_sampler;\nlayout(location=0) out vec4 color;\nvoid main(){color=vec4(texture(sampler2D(previous,previous_sampler),gl_FragCoord.xy/vec2(textureSize(sampler2D(previous,previous_sampler),0))).rgb*0.95+vec3(0.01,0.02,0.03),1.0);}')
            package,clip = root/'feedback.shade',root/'feedback.mp4'
            self.invoke('pack',root,'--output',package)
            self.invoke('media','render-preview',package,'--video',clip,'--seconds','1','--daemon',DAEMON)
            frames = self.rgb(clip)
            self.assertEqual(len(frames),12)
            self.assertGreater(frames[-1][0],frames[0][0]+3)

    def test_media_import_preserves_original_bytes_and_embeds_real_previews(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root/"source's image.png"
            subprocess.run(['ffmpeg','-nostdin','-v','error','-f','lavfi','-i','color=blue:s=32x32','-frames:v','1',str(source)],check=True)
            package = root/'image.shade'
            self.invoke('media','import',source,'--output',package)
            info = json.loads(self.invoke('media','info',package))
            self.assertEqual(info['preview_renderer'],'kroma')
            self.assertEqual((info['width'],info['height']),(32,32))
            self.assertIsNone(info['video_preview'])
            entry = json.loads(self.invoke('inspect',package,'dump-entry','assets/'+source.name,'--full','--format','json'))
            self.assertEqual(bytes.fromhex(entry['preview_hex']),source.read_bytes())
            self.assertTrue(source.exists())


if __name__ == '__main__':
    unittest.main()
