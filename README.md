# cdbpatch

This allows patching a `compile_commands.json` file which can be necessary when
using `clang-tidy` in a cross-compiler environment.

Features:

- add internal toolchain includes to the compiler command-line. Required when
  the sourcecode is incompatible with clangs libc.
- override compiler. Required when the feature above is needed and the database
  doesn't contain the actual compiler, e.g. because it was created using
  `intercept-build`.
- remove compiler-flags. Required if clang doesn't support all of the gcc flags
- add additional compiler flags. Usually not needed but added for completeness.

## ESP32 example (legacy make)
```bash
cdbpatch \
    --use-cc xtensa-esp32-elf-gcc \
    --use-cxx xtensa-esp32-elf-g++ \
    --resolve-toolchain-includes \
    --ccdel=-mlongcalls \
    --ccdel=-fstrict-volatile-bitfields \
    -o compile_commands.json \
    compile_commands-orig.json
```
