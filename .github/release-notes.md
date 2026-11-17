## Download

| File | For |
|---|---|
| `Logloupe_<version>_x64-setup.exe` | The desktop app for Windows 10 and 11 (64-bit). Installs for the current user; no administrator rights needed. |
| `logloupe-cli-<version>-windows-x64.zip` | The command-line tool for Windows. |
| `logloupe-cli-<version>-linux-x64.tar.gz` | The command-line tool for Linux (x86-64, Ubuntu 22.04 or newer and equivalents). |
| `logloupe-cli-<version>-macos-arm64.tar.gz` | The command-line tool for macOS on Apple silicon. |
| `SHA256SUMS.txt` | SHA-256 checksums of the files above. |

The desktop app is only built for Windows so far.

## The installer is not signed yet

Windows SmartScreen will say "Windows protected your PC" the first time you run the installer. Choose **More info**, then **Run anyway**. You can check the download against `SHA256SUMS.txt` first:

```
certutil -hashfile Logloupe_<version>_x64-setup.exe SHA256
```

To remove the app, use **Settings > Apps > Installed apps > Logloupe > Uninstall**. Your sites and settings are kept in `%APPDATA%\logloupe` unless you delete that folder too.

To update, download the newer installer and run it over the old one. Your data is kept.

## Code signing policy

Releases are built by GitHub Actions from the tagged source. The project is applying to the SignPath Foundation programme for open source projects; once approved, releases will carry free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org). The full [code signing policy](https://github.com/logloupe/logloupe#code-signing-policy), with team roles and the privacy statement, is in the README.

See the [README](https://github.com/logloupe/logloupe#readme) for how to use it and the privacy notes.
