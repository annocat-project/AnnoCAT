# Installation and updates

## Install a release

1. Download the Windows ZIP from
   [GitHub Releases](https://github.com/annocat-project/AnnoCAT/releases).
2. Extract the complete ZIP to one folder.
3. Run `launch-annocat.cmd`.
4. Keep the terminal open while AnnoCAT is running.

Do not run AnnoCAT from inside the ZIP. The application, fastVEP, runtime
libraries, configuration, annotation data, downloads, and results use the
extracted folder as one portable installation.

## First launch

Choose one of these actions:

- **Core annotation** installs the GRCh38 reference and Ensembl transcript data
  needed for consequence annotation.
- **Set up offline annotation** opens Data sources so you can install a profile
  or individual local sources.
- **Open results** reviews an existing AnnoCAT result without installing
  annotation data.

Large sources can require substantial download time and storage. The Data
sources page shows the available size information before installation. AnnoCAT
verifies prepared files before it marks a source as ready.

## Move an installation

Stop AnnoCAT before moving or copying its complete extracted folder. Keep the
folder structure intact. AnnoCAT stores managed locations relative to its home
folder where possible and repairs supported older metadata when the new home is
opened.

Use **Data sources > Verify** or `annocat sources verify` after copying large
prepared caches. Verification reads the installed artifacts but does not change
existing results.

## Update AnnoCAT

Stop the running application, extract the new release, and keep a backup of the
existing portable folder until the new build opens successfully. Annotation
source updates are separate from application updates. Use **Check for updates**
on the Data sources page to review source releases.

Existing results are self-contained and are not reannotated when a source is
updated. A new annotation uses the currently installed source release.
