import { MonitorCog } from "lucide-react";
import { useState } from "react";

const systems = [
  ["alma", "os-alma.svg", "AlmaLinux"], ["alpine", "os-alpine.webp", "Alpine Linux"],
  ["arch", "os-arch.svg", "Arch Linux"], ["armbian", "os-armbian.png", "Armbian"],
  ["centos", "os-centos.svg", "CentOS"], ["debian", "os-debian.svg", "Debian"],
  ["fedora", "os-fedora.svg", "Fedora"], ["gentoo", "os-gentoo.svg", "Gentoo"],
  ["red hat", "os-redhat.svg", "Red Hat"], ["rhel", "os-redhat.svg", "Red Hat"],
  ["mint", "os-mint.svg", "Linux Mint"], ["manjaro", "os-manjaro.svg", "Manjaro"],
  ["kali", "os-kali.svg", "Kali Linux"], ["openwrt", "os-openwrt.svg", "OpenWrt"],
  ["istore", "os-istore.png", "iStoreOS"], ["opencloud", "os-opencloud.svg", "OpenCloudOS"],
  ["ubuntu", "os-ubuntu.svg", "Ubuntu"], ["rocky", "os-rocky.svg", "Rocky Linux"],
  ["oracle", "os-oracle.svg", "Oracle Linux"], ["suse", "os-opensuse.svg", "openSUSE"],
  ["nix", "os-nix.svg", "NixOS"], ["proxmox", "os-proxmox.ico", "Proxmox VE"],
  ["synology", "os-synology.ico", "Synology DSM"], ["macos", "os-macos.svg", "macOS"],
  ["darwin", "os-macos.svg", "macOS"], ["windows", "os-windows.svg", "Windows"],
] as const;

function systemInfo(os: string | null | undefined) {
  const source = (os || "").toLowerCase();
  const match = systems.find(([keyword]) => source.includes(keyword));
  return match ? { file: match[1], name: match[2] } : { file: "os-unknown.svg", name: os || "Linux" };
}

export function OSIcon({ os, size = 16 }: { os: string | null | undefined; size?: number }) {
  const [failed, setFailed] = useState(false);
  const info = systemInfo(os);
  if (failed) return <MonitorCog size={size} className="os-icon-fallback" aria-label={info.name} />;
  return <img className="os-icon" src={`/os-icons/${info.file}`} width={size} height={size} alt={info.name} title={info.name} loading="lazy" onError={() => setFailed(true)} />;
}
