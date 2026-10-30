# Packaging and releases

| Platform | Command (from the repository root) | Result in `dist/` |
|---|---|---|
| Windows | `powershell -File packaging\windows\build.ps1` | `Alfadoc-<v>-windows-portable.zip`, and `Alfadoc-<v>-setup.exe` when Inno Setup 6 is installed |
| macOS | `bash packaging/macos/build.sh` | `Alfadoc-<v>-macos.zip` containing an unsigned `Alfadoc.app` |
| Linux | `bash packaging/linux/build.sh` | `alfadoc-<v>-linux-x86_64.tar.gz` (binary, `.desktop` entry, icon, file type) |

The version comes from `version` in the root `Cargo.toml`.

## Windows installer

Install [Inno Setup 6](https://jrsoftware.org/isinfo.php) once. The installer needs no administrator rights by default, adds a Start-menu entry (and an optional desktop shortcut) and makes double-clicking a `.alfadoc` file open it in Alfadoc. A release exe has no console window and carries the Alfadoc icon.

Windows SmartScreen warns about unsigned installers. To remove the warning, sign `alfadoc.exe` and the setup file with a code-signing certificate (`signtool`); that is a purchase and a build step still to add.

## macOS

The app bundle is unsigned. On another Mac, the first start needs right-click → Open. Signing and notarising needs an Apple Developer account.

## Releases from GitHub

`.github/workflows/release.yml` builds all three when a tag such as `v0.1.0` is pushed:

```powershell
git tag v0.1.0
git push origin v0.1.0
```

The workflow checks out the three engines beside Alfadoc. Edit the three `*_REPO` values at the top of the file to where your copies live, and add a repository secret `ENGINES_TOKEN` if they are private. It uses each engine's latest commit; to pin them to the revisions Alfadoc was tested with, add `--branch <tag>` or a `git checkout <rev>` after each clone.

## Opening a file from the command line

`alfadoc path\to\file.alfadoc` opens it on start; `--tab sheet|doc|slides|data` picks the first tab.
