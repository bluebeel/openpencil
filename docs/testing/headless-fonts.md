# Exact font files for headless services

Local and managed `--serve-web`, `--mcp` and `--mcp-http` startup can load exact
font files before accepting requests. Set `OPENPENCIL_FONT_FILES` to a platform
path list (colon-separated on macOS/Linux, semicolon-separated on Windows).
For example:

```sh
OPENPENCIL_FONT_FILES="/fonts/Roboto.sfnt:/fonts/Inter.otf" \
  target/debug/op-host-web-server --serve-web --managed --port 0 --file /design.op
```

Files are read in the supplied order, canonical path aliases are deduplicated,
and the existing imported-font registry supplies their bytes to Native font
resolution. Select the intended font revision explicitly. A directory containing
several revisions does not define which source a document requires.

Configured input is bounded to 64 files / 64 MiB. Missing/unreadable/invalid or
over-budget input fails startup with a diagnostic. An unset or empty list keeps
the previous embedded/system fallback behavior. This setting applies to local
and managed modes; online deployment configuration stays separate.

The real-file fixture proof selects three font files per document. MUI's eight
available requests and OpenBridge's nine requests resolve to the selected
SFNT tables without synthetic styles. MUI Material Icons is still missing.
Canonical imports do not retain every original font revision, so these selected
resource proofs do not establish whole-document font parity or per-document
revision isolation. See [operation coverage](operation-coverage.md).
