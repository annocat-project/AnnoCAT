# Command line

The CLI uses the same portable home, annotation sources, results, and recovery
state as the desktop application.

```text
annocat --help
annocat <COMMAND> --help
```

Use `--home <FOLDER>` to select another portable AnnoCAT home. Commands use the
default home when this option is omitted.

## Common commands

```text
annocat status --profile standard
annocat sources list
annocat sources install --profile standard
annocat annotate -i sample.vcf.gz --profile standard
annocat results list
annocat tasks list
annocat launch
```

`annotate` requires exactly one source mode:

- `--profile standard`, `--profile comprehensive`, or `--profile online`;
- one or more `--source <SOURCE>` options; or
- `--core-only`.

Repeat `-i` for a sequential batch. Use `--include-annotated-vcf` to retain the
annotated VCF in addition to the structured result. Use `--confirm-grch38` only
when the file is known to use GRCh38 and its header does not identify the
assembly.

## Results

```text
annocat results list
annocat results show <RESULT>
annocat results export <RESULT> -o <ZIP>
annocat results import <ZIP>
annocat results validate <RESULT>
```

`results validate` checks files, schemas, and hashes. It does not modify or
reannotate the result.

## Sources and tasks

```text
annocat sources status <SOURCE>
annocat sources fields <SOURCE>
annocat sources verify <SOURCE>
annocat sources remove <SOURCE>
annocat tasks show <TASK>
annocat tasks resume <TASK>
annocat tasks cancel <TASK>
```

Removing a source does not change existing results. Canceling a recoverable
task discards that task's partial data.

Use `--json` where shown by command help when a script needs machine-readable
output. Human-readable output is not a stable parsing interface.
