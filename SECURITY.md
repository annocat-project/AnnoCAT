# Security policy

Report a suspected vulnerability through the repository's private GitHub
security advisory form. Do not include private genomic data, credentials, or
other sensitive records in a report. Do not open a public issue until the
problem has been assessed.

Include the affected AnnoCAT version, the operating system version, a minimal
reproduction, and the expected security boundary. Use fabricated input when a
file is needed to reproduce the problem.

AnnoCAT treats imported result ZIPs and downloaded annotation data as untrusted
input. The current import boundary is documented in
[`docs/result-import-security.md`](docs/result-import-security.md).
