# Security policy

SpaceMosaic is pre-release software. Security fixes target the current main
branch; no stable release series or response-time guarantee is established.

## Reporting a vulnerability

Use **Security → Report a vulnerability** on GitHub when available, or the
repository owner's published GitHub profile contact. Do not publish exploit
material containing private file names, paths, content URIs, grants, or signing
credentials. Coordinate disclosure before posting sensitive details publicly.

## Security boundaries

The application operates with ordinary user permissions. It does not delete
files, execute user-defined commands, upload metadata, or request all-files
access on Android. Native providers control the scope available to the app.
A cloud provider may use its own network connection to answer metadata queries.

JSON exports include file names, display paths, and lossless native path
representations. CSV exports contain file-type summaries and protect extension
fields against spreadsheet formula interpretation. Treat exports and screenshots
as potentially private information.

Do not commit tokens, private keys, debug or release keystores, certificates,
or provisioning profiles. Dependabot security updates are not disabled by the
repository configuration. Repository administrators control GitHub security
alerts and private reporting availability.
