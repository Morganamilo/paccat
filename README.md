# Paccat

Print pacman package files

# Usage

```
    paccat [options] <target> <files>
    paccat [options] <targets> -- <files>
    paccat [options] -<Q|F> <files>
    paccat [options] -<Q|F> [targets] -- <files>

```

a target can be specified as `<pkgname>`, `<repo>/<pkgname>`, `<url>` or `<file>`.

Files can be specified as just the filename or the full path.

## Examples

`paccat grub etc/default/grub`

Print the contents of 'etc/default/grub' from the grub package.

---

`paccat pacman pacman.conf`

Print the contents of the first file named 'pacman.conf' from the pacman package.

---

`paccat -F pacman.conf`

Print the contents of the first file named 'pacman.conf' in the repos

---

`paccat -x pacman mkinitcpio -- '\\.conf$'`

Print the contents of all files ending in '.conf' from both the pacman and mkinitcpio packages.

---

`paccat ~/pkgs/my-pkg-1.0.0-1.pkg.tar.zst myfile`

Print the contents of 'myfile' from a package tarball.

---

`paccat https://archlinux.org/packages/extra/x86_64/git/download git-blame.1.gz`

Download and print the contents of 'git-blame.1.gz' from the git package.";
