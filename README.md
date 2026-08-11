# PlayStation Vita tool

Tool for video game archival and analysis of PS Vita cartridge dumps.

### Extraction
Extracts files from .img/.vci/.psv cart images, creates a zstd-compressed skeleton, and a list of file hashes/sizes/offsets.

usage: `petra.exe example.img`

### Rebuilding
Rebuilds a .img file given a folder of files, the original skeleton (`example.skeleton.zst`), and the list of file hashes (`example.files.tsv`).

usage: `petra.exe example/`

You can also force rebuilding the img with a different license .rif file even if it doesn't match the provided hash.

usage: `petra.exe example/ license.rif`

### Validation
Validate a folder of files given an existing list of file hashes (`example.files.tsv`)

usage: `petra.exe example/`

**Note**: It is expected that the below three files will be missing from most file-only dumps made on consoles:
- /gc/param.sfo
- /license/app/\*/\*.rif
- /psp2/update/psp2updat.pup
