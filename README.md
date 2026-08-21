# petra

PlayStation Vita archival tool for extracting/transforming/rebuilding/analyzing physical game card dumps.  
Dumps of the same game from different sources or file formats should result in identical outputs (skeleton / file list / extracted files).  
You can also use this tool to easily compare and verify PSN game dumps with game files on physical dumps.

## Extraction
Extracts files from .img/.vci/.psv images, creates a zstd-compressed skeleton, and a list of file hashes/sizes/offsets.

usage: `petra.exe example.img`

**Note**: The skeleton's image header and extracted RIF file have their unique data zeroed to be deterministic.

## Rebuilding
Rebuilds a raw img file given a folder of files, a skeleton (`example.skeleton.zst`), and a list of file hashes (`example.tsv`).

usage: `petra.exe example/`

You can also force rebuilding the img with a different license .rif file even if it doesn't match the provided hash (such as a cleaned/fake license).

usage: `petra.exe example/ license.rif`

## Validation
Validate a folder of files given an existing list of file hashes. Ensure the filename matches the folder name, e.g. `example.tsv` and `example/`. This also work with PSN dumps (including NoNpDrm dumps), just point to the folder of files (no need to rename the files).

usage: `petra.exe example/`

**Note**: It is expected that the below three files will be missing from PSN dumps and most file-only dumps made on consoles (including NoNpDrm format):
- `/gc/param.sfo` (Game card metadata)
- `/license/app/*/*.rif` (License file, often referred to as work.bin) 
- `/psp2/update/psp2updat.pup` (PSVita system update file)
