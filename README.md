# PlayStation Vita tool

PS Vita archival tool for extracting/transforming/rebuilding/analyzing cartridge dumps.  
Dumps of the same game from different sources or file formats should result in identical outputs (skeleton / file list / extracted files).  
You can also easily compare PSN dumps with game files on cartridge dumps.

## Extraction
Extracts files from .img/.vci/.psv cart images, creates a zstd-compressed skeleton, and a list of file hashes/sizes/offsets.

usage: `petra.exe example.img`

**Note**: The skeleton's image header and extracted RIF file have their unique data zeroed to be deterministic.

## Rebuilding
Rebuilds a .img file given a folder of files, the original skeleton (`example.skeleton.zst`), and the list of file hashes (`example.files.tsv`).

usage: `petra.exe example/`

You can also force rebuilding the img with a different license .rif file even if it doesn't match the provided hash.

usage: `petra.exe example/ license.rif`

## Validation
Validate a folder of files given an existing list of file hashes (`example.files.tsv`). This also work with PSN dumps (including NoNpDrm dumps), just point to the folder of files.

usage: `petra.exe example/`

**Note**: It is expected that the below three files will be missing from PSN dumps and most file-only dumps made on consoles (including NoNpDrm format):
- /gc/param.sfo
- /license/app/\*/\*.rif
- /psp2/update/psp2updat.pup
