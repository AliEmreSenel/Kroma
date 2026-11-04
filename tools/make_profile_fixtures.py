#!/usr/bin/env python3
"""Create reproducible v2 shader, feedback, lifecycle, and embedded-video workloads."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess


def generate(cli, output, video=None):
    """Build fixtures using the real importer and packer, never editing source media."""
    cli = cli.resolve()
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    examples = Path(__file__).resolve().parents[1] / 'examples/shadertoy'
    shutil.copyfile(examples / 'plasma.glsl', output / 'plasma.glsl')

    def command(*args):
        return subprocess.check_output([str(cli), *map(str, args)], text=True)

    command('import', output / 'plasma.glsl', '--name', 'Profile plasma', '--author', 'Kroma')
    entry = json.loads(command('inspect', output / 'plasma.shade', 'dump-entry', 'shader.frag', '--full', '--format', 'json'))
    shader = bytes.fromhex(entry['preview_hex'])
    feedback = b'''#version 450
layout(set=1,binding=0) uniform texture2D previous;
layout(set=1,binding=1) uniform sampler previous_sampler;
layout(location=0) out vec4 color;
void main(){ color=vec4(texture(sampler2D(previous,previous_sampler),gl_FragCoord.xy/vec2(textureSize(sampler2D(previous,previous_sampler),0))).rgb*0.95+vec3(0.01,0.02,0.03),1.0); }
'''
    definitions = {
        'phases': ''.join(f'[states.{phase}]\nlength={length}\nshader="plasma.frag"\n' for phase, length in [('load', 0.05), ('active', 0), ('unload', 0.05)]),
        'nested': '[states.active]\nlength=0\n[states.active.textures.scene]\ntype="shader"\nshader="plasma.frag"\nwidth=1920\nheight=1080\n',
        'feedback': '[states.active]\nlength=0\n[states.active.textures.scene]\ntype="shader"\nshader="feedback.frag"\nwidth=1920\nheight=1080\n[states.active.textures.scene.textures.previous]\ntype="shader"\ninput="t-1"\n',
    }
    if video:
        definitions['embedded-video'] = '[states.active]\nlength=0\n[states.active.textures.video]\ntype="video"\nsource="video.mp4"\nlooping=true\n'
    sequence = [{'label': 'plasma', 'path': str(output / 'plasma.shade'), 'seconds': 5, 'reloads': 3}]
    for name, config in definitions.items():
        source = output / name
        source.mkdir()
        (source / 'config.toml').write_text(f'[meta]\nname={json.dumps(name)}\nauthor="Kroma"\n' + config)
        (source / 'plasma.frag').write_bytes(shader)
        (source / 'feedback.frag').write_bytes(feedback)
        if name == 'embedded-video':
            shutil.copyfile(video, source / 'video.mp4')
        package = output / f'{name}.shade'
        command('pack', source, '--output', package)
        sequence.append({'label': name, 'path': str(package), 'seconds': 5, 'reloads': 3})
    (output / 'sequence.json').write_text(json.dumps(sequence, indent=2) + '\n')
    return output / 'sequence.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True, help='existing kroma CLI executable')
    parser.add_argument('--output', type=Path, required=True, help='new fixture directory')
    parser.add_argument('--video', type=Path, help='optional real video to embed')
    args = parser.parse_args()
    print(generate(args.cli, args.output, args.video))


if __name__ == '__main__':
    main()
