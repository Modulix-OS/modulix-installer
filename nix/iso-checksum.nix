{ runCommand, isoImage }:

runCommand "modulixos-installer-iso" { } ''
  mkdir -p $out/iso
  for f in ${isoImage}/iso/*; do
    ln -s "$f" "$out/iso/$(basename "$f")"
  done

  cd $out/iso
  for f in *.iso *.iso.zst; do
    [ -e "$f" ] || continue
    sha256sum "$f" > "$f.sha256"
  done

  mkdir -p $out/nix-support
  cp --no-preserve=mode ${isoImage}/nix-support/* $out/nix-support/
  for f in $out/iso/*.sha256; do
    echo "file sha256 $f" >> $out/nix-support/hydra-build-products
  done
''
