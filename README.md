# petra

PlayStation Vita archival tool for extracting/transforming/rebuilding/analyzing physical game card dumps.  
You may use this tool to losslessly normalize your game card dumps to formats ideal for archival.  
It also supports file extracting, leaving behind a filesystem skeleton (dump image with all game data zeroed).
You can also use this tool to easily compare and verify PSN game dumps with game files on physical dumps.

## Extraction
Extracts files from .psv/.vci/.img dump images, creates a zstd-compressed skeleton, and a list of file hashes/sizes/offsets.

usage: `petra.exe extract example.img`

**Note**: The skeleton and extracted license file have their unique data zeroed to be deterministic. The unique data are stored as sidecar files, just like the `strip` command. The output files will overwrite any existing files.

## Normalizing
Strips non-deterministic data from .img/.vci/.psv images, for normalization and allowing comparison between multiple dumps.

usage: `petra.exe strip example.img`

**Note**: This will extract the unique data from the PSV/VCI header (.hdr), unknown header data (.unk), license (.rif) file, and BlackFin specific data (.blackfin) if it is present. The output files will overwrite any existing files.

## Repairing
Applies given sidecar files (.hdr/.unk/.rif/.blackfin) to a normalized/stripped image, reverting back to the original unique dump file.

usage: `petra.exe repair example.img`

## Rebuilding
Rebuilds a raw img file given a folder of files, a skeleton (`example.skeleton.zst`), and a list of file hashes (`example.tsv`).

usage: `petra.exe rebuild example/`

**Note**: This will not repair the dump with the original unique data (will not apply the sidecar files even if they exist). Use the `repair` function after rebuilding if this is needed.

## Analysis
Performs checks and prints info, warnings, and game metadata without writing to any files.

usage: `petra.exe analyze example.img`

## Verification
Verify a folder of files given an existing list of file hashes. Ensure the filename matches the folder name, e.g. `example.tsv` and `example/`. This also work with PSN dumps (including NoNpDrm dumps), just point to the folder of files (no need to rename the files).

usage: `petra.exe verify example/`

**Note**: It is expected that the below three files will be missing from PSN dumps and most file-only dumps made on consoles (including NoNpDrm format):
- `/gc/param.sfo` (Game card metadata)
- `/license/app/*/*.rif` (License file, often referred to as work.bin) 
- `/psp2/update/psp2updat.pup` (PSVita system update file)
