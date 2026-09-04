#!/bin/bash
# Every tool, one process at a time, order REVERSED on alternate passes so no
# tool keeps the same predecessor. This origin throttles per client IP, so a tool
# that always follows a heavy multi-connection run would be charged for it.
set -u
URL="https://ash-speed.hetzner.com/1GB.bin"
HY=${HYDRA:-hydra}
D=/tmp/bm
COOL=${COOL:-10}
PASSES=${PASSES:-4}
OUT=/root/all.csv
REF=9fd9dd5f52df2974eb7273c14fc2aa6bce71f3b4dc1f704e6875a87ae27e90a0
mkdir -p $D
echo "app,config,pass,seconds,bytes,cpu_seconds,max_rss_kb,checksum" > $OUT

run_one() { # pass app config cmd...
  local pass=$1 app=$2 cfg=$3; shift 3
  rm -f $D/*
  /usr/bin/time -f "%e %U %S %M" -o /tmp/m.txt "$@" >/dev/null 2>/tmp/tool.log
  read real user sys rss < /tmp/m.txt
  local f=$(ls $D 2>/dev/null | head -1)
  local bytes=$(stat -c %s "$D/$f" 2>/dev/null || echo 0)
  local sha=$(sha256sum "$D/$f" 2>/dev/null | cut -c1-64)
  local ok=no; [ "$sha" = "$REF" ] && ok=ok
  local cpu=$(awk "BEGIN{printf \"%.2f\", $user+$sys}")
  echo "$app,$cfg,$pass,$real,$bytes,$cpu,$rss,$ok" >> $OUT
  printf "  %-18s %8ss  cpu %5ss  rss %6s MiB  %s\n" "$app $cfg" "$real" "$cpu" \
    "$(awk "BEGIN{printf \"%.1f\", $rss/1024}")" "$ok"
  sleep $COOL
}

c_curl(){ run_one $1 curl   "1 conn"   curl -s -o $D/f.bin "$URL"; }
c_wget(){ run_one $1 wget   "1 conn"   wget -q -O $D/f.bin "$URL"; }
c_axel(){ run_one $1 axel   "-n 4"     axel -q -n 4 -o $D/f.bin "$URL"; }
c_a1(){   run_one $1 aria2c "-x 1"     aria2c -q -x1 -s1 --file-allocation=none --allow-overwrite=true -d $D -o f.bin "$URL"; }
c_a4(){   run_one $1 aria2c "-x 4"     aria2c -q -x4 -s4 --file-allocation=none --allow-overwrite=true -d $D -o f.bin "$URL"; }
c_a8(){   run_one $1 aria2c "-x 8"     aria2c -q -x8 -s8 --file-allocation=none --allow-overwrite=true -d $D -o f.bin "$URL"; }
c_h1(){   run_one $1 hydra  "-x 1"     $HY -q -x 1 -O $D/f.bin "$URL"; }
c_h2(){   run_one $1 hydra  "-x 2"     $HY -q -x 2 -O $D/f.bin "$URL"; }
c_h4(){   run_one $1 hydra  "-x 4"     $HY -q -x 4 -O $D/f.bin "$URL"; }
c_h8(){   run_one $1 hydra  "-x 8"     $HY -q -x 8 -O $D/f.bin "$URL"; }
c_had(){  run_one $1 hydra  "adaptive" $HY -q --adaptive -x 8 -O $D/f.bin "$URL"; }
c_hdef(){  run_one $1 hydra  "default"  $HY -q -O $D/f.bin "$URL"; }

fwd(){ c_curl $1; c_wget $1; c_axel $1; c_a1 $1; c_a4 $1; c_a8 $1; c_hdef $1; c_h1 $1; c_h2 $1; c_h4 $1; c_h8 $1; c_had $1; }
rev(){ c_had $1; c_h8 $1; c_h4 $1; c_h2 $1; c_h1 $1; c_hdef $1; c_a8 $1; c_a4 $1; c_a1 $1; c_axel $1; c_wget $1; c_curl $1; }

echo "warm-up (discarded)"
curl -s -o $D/f.bin "$URL" >/dev/null 2>&1; rm -f $D/*
sleep $COOL
for p in $(seq 1 $PASSES); do
  echo "== pass $p of $PASSES =="
  if [ $((p % 2)) -eq 1 ]; then fwd $p; else rev $p; fi
done
rm -rf $D
echo DONE
