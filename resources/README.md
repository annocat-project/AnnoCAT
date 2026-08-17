# Prepared annotation data

AnnoCAT stores managed references and prepared annotation sources in this
directory at runtime. The large generated contents are ignored by Git.

Install, update, verify, and remove these files through the Data sources page or
the `annocat sources` commands. Do not edit prepared shards or manifests by
hand. Existing results are self-contained and are not changed when a source is
updated or removed.

Only public checksum files required by a versioned source contract belong in
the repository. Do not commit downloaded archives, prepared caches, partial
downloads, credentials, or private data.
