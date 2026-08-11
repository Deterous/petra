# PlayStation Vita tool

Tool for video game archival and analysis of PS Vita cartridge dumps.

### PSV extraction
Extracts files from a decrypted raw image, creates a zstd-compressed skeleton, and a list of file hashes/sizes/offsets.

usage: `petra.exe example.img`

### PSV rebuilding
Rebuilds a img file given a folder of files, the original skeleton (`example.skeleton.zst`), and the list of file hashes (`example.files.tsv`).

usage: `petra.exe example/`

### File validation
Validate a folder of files given an existing list of file hashes (`example.files.tsv`)

usage: `petra.exe example/`

**Note**: It is expected that the below three files will be missing from most file-only dumps made on consoles:
- /gc/param.sfo
- /license/app/\*/\*.rif
- /psp2/update/psp2updat.pup
