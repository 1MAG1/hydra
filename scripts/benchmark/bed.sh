#!/bin/bash
set -u
U=${U:-http://10.99.0.2:8099/1GB.bin}
SZ=${SZ:-1073741824}
HY=${HYDRA:-hydra}
D=/mnt/ram/out; mkdir -p $D
m(){ local label="$1"; shift; rm -f $D/*
  /usr/bin/time -f "%e %U %S %M" -o /tmp/m.txt "$@" >/dev/null 2>&1
  read e u s r < /tmp/m.txt
  awk -v l="$label" -v e="$e" -v u="$u" -v s="$s" -v r="$r" -v sz="$SZ" "BEGIN{printf \"  %-16s %7.2fs %8.1f MB/s  cpu %5.2fs  rss %6.1f MiB\n\", l, e, sz/e/1e6, u+s, r/1024}"
  sleep 1; }
for rep in 1 2 3; do
echo "== rep $rep =="
m "curl"        curl -s -o $D/f.bin $U
m "wget"        wget -q -O $D/f.bin $U
for n in 1 2 4 8; do m "aria2c -x$n" aria2c -q -x$n -s$n --file-allocation=none --allow-overwrite=true -d $D -o f.bin $U; done
for n in 1 2 4 8; do m "hydra -x$n"  $HY -q -x $n -O $D/f.bin $U; done
m "hydra default" $HY -q -O $D/f.bin $U; m "hydra adaptive" $HY -q --adaptive -x 8 -O $D/f.bin $U
done
rm -rf $D
