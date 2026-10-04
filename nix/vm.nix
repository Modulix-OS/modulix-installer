{ pkgs, isoImage }:

let
  diskImg = "modulixos-vm-disk.qcow2";
  tpmDir = "modulixos-vm-tpm";
in
pkgs.writeShellScriptBin "modulixos-vm" ''
  set -euo pipefail

  rm -f ${diskImg}
  ${pkgs.qemu}/bin/qemu-img create -f qcow2 ${diskImg} 40G

  rm -rf ${tpmDir}
  mkdir -p ${tpmDir}
  ${pkgs.swtpm}/bin/swtpm socket \
    --tpmstate dir=${tpmDir} \
    --ctrl type=unixio,path=${tpmDir}/swtpm-sock \
    --tpm2 \
    --daemon

  res=""
  display_opts=""
  if [ -n "''${MODULIXOS_VM_RES:-}" ]; then
    res=",xres=''${MODULIXOS_VM_RES%%x*},yres=''${MODULIXOS_VM_RES##*x}"
    display_opts=",zoom-to-fit=on"
  fi

  if [ "''${MODULIXOS_VM_GL:-1}" = 0 ]; then
    gpu=(-device "virtio-vga$res" -display "gtk$display_opts")
  else
    gpu=(-device "virtio-vga-gl$res" -display "gtk,gl=on$display_opts")
  fi

  ${pkgs.qemu}/bin/qemu-system-x86_64 \
    -enable-kvm \
    -m 8192 \
    -smp 4 \
    -cpu host \
    -drive if=pflash,format=raw,readonly=on,file=${pkgs.OVMF.firmware} \
    -drive if=virtio,file=${diskImg},format=qcow2 \
    -cdrom ${isoImage}/iso/*.iso \
    -boot d \
    "''${gpu[@]}" \
    -chardev socket,id=chrtpm,path=${tpmDir}/swtpm-sock \
    -tpmdev emulator,id=tpm0,chardev=chrtpm \
    -device tpm-tis,tpmdev=tpm0 \
    "$@"
''
