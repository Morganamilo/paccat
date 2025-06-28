# Changelog

## Paccat v1.4.0 (2025-06-28)

### Added

- Add debug option 2922f38

### Fixed

- Make sure target is not a dir b46f7a0
- Error if files are empty bf65e74
- Don't warn about missing db if refreshing 145216f

### Changed

- Improve sytax highlighting 321985f
- Allow refresh without targets 5112e43
- Print when starting package download 08fd48f
- Preappend the cache dir specified with --cachedir b54acbc
- Add uid to temp download dir 67ec340

## Paccat v1.3.1 (2024-12-27)

### Fixed

- Unset sandbox user if not root d4722ef

## Paccat v1.3.0 (2024-08-28)

### Changed

- Update alpm b0ba6c6

## Paccat v1.2.0 (2024-03-10)

### Added

- Match regex once without -a 966c5d1

### Changed

- Add * for match all 28e9ae7
- Ignore symlinks e51951d

## Paccat v1.1.0 (2024-01-02)

### Fixed

- Fix extraction of binary files adf40cb
- Handle EPIPE instead of relying on sigpipe 5231c12

### Changed

- Adjust examples c0a8adb
- Only download first package with -F/-Q 194c332
- Allow ommitting targets with -Q/-F bbff82c
- Rename quiet to list f6ff71e

## Paccat v1.0.0 (2023-12-4)

### Added

- Alow reading from stdin bc7920d
- Add root warning if refresh fails e79254d
- Update examples 04458cc
- Fix colour printing 8c6cb79
- Don't pass binary files to bat 597654b
- Add --color 1761bb8
- Cargo clippy 04bef96
- Use bat for syntax highlighting 8bc6a5f
- only chown if root 576cb4b
- Add cache dirs after default 476f683
- Verify packages 8959a8c
- Use pacman cache dirs as readonly cache 689e30d
- Check localdb before downloading e5a78ec
- Add option to refresh databases 0d19e25
- Add option to install file bad69eb
- Allow not using -- for one package 5af1895
- Give pacman like error message for no targets/files 6246ce0
- Complete file names on args 002c8ad

## Paccat v0.2.0 (2021-11-01)

### Added

- Add man page #18
- Add completion #16
- Implement -e/--extract #13
- Add warning for missing databases d1e7217
- Add ability to search through local or files db with -Q and -F 05c78c6
- Download in parallel b524ede

### Fixed

- Fix typos ad4220c 54b8a4c 7bbba78
- Fix return value c6f743b

### Changed

- Print first match by default with -a to print all 484e71d
- Change tmp download dir to /tmp/paccat c765e95
- Print binary files when piped #10

## Paccat v0.1.0 (2021-10-13)

### Initial Release

