A transmission daemon torrent client

* Fast way to command common torrent actions such as querying/filtering, adding,
  starting, stopping, erasing and removing (with data).

* Fast way to work with torrents in batches.

* Makes it possible to directly send torrents to the transmission daemon from
  your web browser (desktop files included).

* The torrents are added in a stopped state if the disk space would not be
  enough to hold all the downloading torrents when they are finished (taking
  account the chosen files).

* Download directory is prefixed with the torrent hash to avoid name collisions.

* A completion script for transmission daemon is included that moves the files
  from the download directory. Compared to transmission's default behavior it
  does not stop the torrent, avoids overwriting files with the same name, avoids
  filling the destination disk space and uses nicer IO.

# Installing

- Copy aur/PKGBUILD to project root, modify it as need and run `makepkg`

# Setup

Transmission daemon and the client used should use the `torrent` group created.

# Developing

Generate completion file with:
```
trctl gen-completions zsh > _trctl
```

# Similar projects

* [Stig](https://github.com/rndusr/stig) provides both a text user interface and
  a command line interface. Is much more featureful than mine :D. Maybe it's not
  as fast to start as a python client however 😤. Haven't tried it.

* [Tremc](https://github.com/tremc/tremc) is a resourceful curses client. It does not
  provide a command line interface however. I use it when I need to do see more
  information about the torrent or apply some more esoteric actions.

# Attributes

* [Bit torrent icons created by Rahat - Flaticon](https://www.flaticon.com/free-icons/bit-torrent)
