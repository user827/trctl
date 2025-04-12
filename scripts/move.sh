#!/bin/bash
set -eu

PATH=/usr/bin

. /usr/lib/trctl/logging.sh

errok=0


error() {
  local msg="$1" ret="${2:-1}"
  log error "$msg"
  errok=1
  exit "$ret"
}

checkstatus() {
  err=$?
  if [ $err != 0 ] && [ $errok = 0 ]; then
    log error "terminating on error: $err"
  fi
}

copytorrent() {
  local copydir
  # Move when completed because cannot copy magnet links before they are downloaded
  copydir=$torrentroot/torrents

  if [ ! -f "$copydir"/"$hsh".torrent ]; then
    # Don't fail in case we aren't transmission user and don't have permission to transmission's private
    # folder
    cp --no-preserve=mode -- "$torrent_file" "$copydir"/"$hsh".torrent.tmp || return 0
    # http://blog.httrack.com/blog/2013/11/15/everything-you-always-wanted-to-know-about-fsync/
    # have to sync both the fila and its directory entry
    sync -- "$copydir"/"$hsh".torrent.tmp
    mv -- "$copydir"/"$hsh".torrent.tmp "$copydir"/"$hsh".torrent
    sync -- "$copydir"
  fi
}


cfgpath=$TR_CONFIG_PATH
torrentroot=$TR_TORRENT_ROOT
lockfiles=$torrentroot/locks
name=$TR_TORRENT_NAME
basedir=$TR_TORRENT_DIR
basepath=$basedir/$name
hsh=$TR_TORRENT_HASH
torrent_file=$TR_TORRENT_FILE
destdir=$TR_TORRENT_DESTINATION
freespacetoleave=$TR_FREE_SPACE_TO_LEAVE
force=$TR_FORCE
verify=$TR_VERIFY


trap checkstatus 0
# some of these needs to be able to cd to current dir
cd /
umask 007
# no permission when resuming otherwise... but might not have finished
copytorrent


srcdevid=$(stat -c %d -- "$basepath")
srclockfile=$lockfiles/$srcdevid
exec 8>"$srclockfile"
if ! flock 8; then
  error "cannot acquire srclock for ${srclockfile#"$torrentroot"} (${basepath#"$torrentroot"} $hsh ${destdir#"$torrentroot"})"
fi
# in case we were called again while a move was already in progress
if [ ! -e "$basepath" ]; then
  error "already moved ${basepath#"$torrentroot"}"
fi

dstdevid=$(stat -c %d -- "$destdir")
dstlockfile=$lockfiles/$dstdevid
# only one concurrent move per destdir so that we can more correctly calculate freespace
exec 9>"$dstlockfile"
if ! flock 9; then
  error "cannot acquire dstlock for ${dstlockfile#"$torrentroot"} (${basepath#"$torrentroot"} $hsh ${destdir#"$torrentroot"})"
fi


size=$(du -B1 -s -- "$basepath" | cut -d'	' -f1)
freespace=$(df -B1 --output=avail -- "$destdir" | tail -1)
if [ "$freespace" -le "$((size + freespacetoleave))" ] && [ "$force" != 1 ]; then
  log warn "not enough space in ${destdir#"$torrentroot"/} for ${basepath#"$torrentroot"/}"
  errok=1
  exit 1
fi


touch -- "$destdir/$hsh.incomplete"

origdestdir=$destdir
destdir=$destdir/$hsh
if [ ! -d "$destdir" ]; then
  mkdir -- "$destdir"
  sync -- "$origdestdir"
fi

#nice -n 19 ionice -c idle cp -a --no-preserve=mode --reflink=auto -t "$destdir" -- "$basepath"
#nice -n 10 cp -a --no-preserve=mode --reflink=auto -t "$destdir" -- "$basepath"
nice -n 10 rsync -a --append -- "$basepath" "$destdir"/

if [ ! -e "$origdestdir/${name}" ]; then
  mv -t "$origdestdir" -- "$destdir/$name"
  rmdir -- "$destdir"
  destdir=$origdestdir
fi

find -- "$destdir/${name}" -exec sync -- '{}' \+
sync -- "$destdir"

# https://trac.transmissionbt.com/ticket/1753
for try in $(seq 3); do
  ret=0
  # blocks the whole process
  trctl --config "$cfgpath" --yes set-location --hsh="$hsh" --location="$destdir" >/dev/null || ret=$?
  #transmission-remote "127.0.0.1:@RPCPORT@" -N /etc/"@NAME@"/netrc -t "$hsh" --find "$destdir" >/dev/null || ret=$?
  [ $ret != 0 ] || break
  if [ "$try" == 3 ]; then
    error "set-location timeout"
  fi
done
# TODO these might not run if the above succeeds but the script dies afterwards...

# TODO
if [ "$verify" = 1 ]; then
    for try in $(seq 3); do
      log notice "start verifying"
      ret=0
      # --verify does not wait for verification to finish
      #transmission-remote "127.0.0.1:@RPCPORT@" -N /etc/"@NAME@"/netrc -t "$hsh" --verify >/dev/null || ret=$?
      trctl --config "$cfgpath" --yes verify --hsh "$hsh" >/dev/null || ret=$?
      [ $ret != 0 ] || break
      if [ "$try" == 3 ]; then
        error "verify start timeout"
      fi
    done
fi

rm --interactive=never -r -- "$basepath"
if [ "${basedir##*/}" = "$hsh" ]; then
  rmdir -- "$basedir"
  sync -- "${basedir%/*}"
else
  sync -- "${basedir}"
fi

rm -- "$origdestdir/$hsh.incomplete"
