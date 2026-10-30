# The `.alfadoc` package — format version 0.1

A zip file. Extension `.alfadoc`, media type `application/vnd.alfadoc+zip`.

| Entry | Required | Contents |
|---|---|---|
| `mimetype` | yes | The media type as ASCII. **Must be the first entry, stored uncompressed**, so tools can identify the file from its first bytes. |
| `manifest.json` | yes | `format` (`"alfadoc"`), `version`, `id` (stable document id), `title`, `created_at`, `updated_at` (ms since epoch), `generator`. |
| `data/maix.json` | yes | The maix store, JSON-records profile (MAIX-1.1 §3). |
| `links.json` | no | Array of links (LINKS-AND-SYNC). Absent = no links. |
| `document/…` | no | Word-processor parts. v1: `document/main.docx` is the WordCraft document, stored as DOCX. |
| `slides/…` | no | Presentation parts. v1: `slides/deck.pptx`. |
| `assets/…` | no | Pictures and other binaries, referenced by content hash. |

**Why DOCX and PPTX inside the package?** WordCraft and DeckCraft already read and write them
faithfully, so v1 does not need a second serialisation of their models. Conversion to Word and
PowerPoint is then a copy plus link flattening. A native serialisation can replace them later
without changing the container; the `version` field covers it.

## Reading rules (all enforced in `alfadoc-model`)

- Entry names are relative, `/`-separated, with no `..`, `.`, empty components, backslashes, drive
  letters or control characters. Anything else is an error (zip-slip).
- At most 20,000 entries; at most 512 MB per entry and 1 GB in total, **measured while streaming**
  (the zip header's claimed size is never trusted).
- `mimetype` must match; `manifest.format` must be `"alfadoc"`.
- Unknown entries outside the known folders are ignored, not an error (forward compatibility).
  Unknown *fields* in JSON are ignored.
- A reader must never panic on any input.

## Writing rules

- Write `mimetype` first and stored. Write the known JSON entries, then parts in name order.
- Parts must live under `document/`, `slides/` or `assets/`; the writer refuses other names.

## Versioning

`manifest.version` is `major.minor`. A reader opens any file with the same major version. A newer
minor adds only optional entries and fields. A different major version is refused with a clear
message rather than opened wrongly. Format migrations use maix's `.mxm` mechanism (MAIX-1.1 §5).

## Planned

- Optional namespace encryption and signature (MAIX §13) in `security/`.
- Computed-value cache in `computed.cache`, safe to delete.
