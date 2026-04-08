# GUI Rewrite Task List

This list tracks GUI work that is intentionally deferred while v2 `.shade` container and runtime migration is implemented.

## Deferred Tasks (Container V2)

1. Add v2 package inspection panel (entry list, codec, chunk count, chunk size).
2. Add `Migrate Legacy .shade` action that calls CLI/IPC migration and surfaces warnings.
3. Add package validation UI for unresolved absolute-path asset references.
4. Show per-entry compression policy in asset inspector (`auto`, `none`, `zstd`, `lz4`).
5. Add pack-preview summary before save: referenced assets, optional missing warnings, deterministic order confirmation.
6. Add user-facing message when loading legacy ZIP `.shade`: suggest one-shot migration.
7. Keep direct single image/video load UX parity with shaderless package behavior (aspect-fill, loop=true for videos).
