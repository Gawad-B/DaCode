import { useEffect, useState } from "react";
import { REPO, REPO_URL } from "./data";

export type Os = "windows" | "linux" | "macos" | "unknown";

export type Download = {
  /** What the button says. */
  label: string;
  /** The asset filename, or a command for platforms with no artifact. */
  detail: string;
  url: string;
  size?: number;
};

export type Platform = {
  os: Os;
  /** The name to print. Not used to choose anything — `os` is. */
  name: string;
  /** null when there is no build for this platform. */
  primary: Download | null;
  /** Other artifacts for the same platform: the rpm and the tarball on Linux. */
  alternatives: Download[];
  /** The line under the button. Honest about what the visitor is getting. */
  note: string;
  /** Shell prompt and command for the install box. */
  prompt: string;
  command: string;
};

export type Release = {
  version: string;
  published: string | null;
  assets: Record<string, { url: string; size: number }>;
  /** True while the real release is still being fetched, or if it failed. */
  stale: boolean;
};

/*
 * The version and the four asset URLs are fetched from the releases API so the
 * page cannot go stale behind a release. It ships knowing 0.4.1 anyway: the API
 * is rate-limited per IP without a token, and a download button that fails
 * closed on the sixty-first visitor of the hour is worse than one link out of
 * date. The fallback is replaced the moment the real answer arrives.
 */
const FALLBACK: Release = {
  version: "0.4.1",
  published: "2026-08-15",
  assets: {
    "vex-setup.exe": {
      url: `${REPO_URL}/releases/download/v0.4.1/vex-setup.exe`,
      size: 7139700,
    },
    "vex_0.4.1_amd64.deb": {
      url: `${REPO_URL}/releases/download/v0.4.1/vex_0.4.1_amd64.deb`,
      size: 9004948,
    },
    "vex-0.4.1-1.x86_64.rpm": {
      url: `${REPO_URL}/releases/download/v0.4.1/vex-0.4.1-1.x86_64.rpm`,
      size: 8902848,
    },
    "vex-0.4.1-x86_64-linux.tar.gz": {
      url: `${REPO_URL}/releases/download/v0.4.1/vex-0.4.1-x86_64-linux.tar.gz`,
      size: 9006859,
    },
  },
  stale: true,
};

type ApiRelease = {
  tag_name?: string;
  published_at?: string;
  assets?: { name?: string; browser_download_url?: string; size?: number }[];
};

export function useRelease(): Release {
  const [release, setRelease] = useState<Release>(FALLBACK);

  useEffect(() => {
    const abort = new AbortController();

    fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      signal: abort.signal,
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((r) => (r.ok ? (r.json() as Promise<ApiRelease>) : Promise.reject(r.status)))
      .then((data) => {
        const assets: Release["assets"] = {};
        for (const a of data.assets ?? []) {
          if (a.name && a.browser_download_url) {
            assets[a.name] = { url: a.browser_download_url, size: a.size ?? 0 };
          }
        }
        // An empty asset list means a release exists but nothing is attached to
        // it yet — keep the links that are known to work rather than offering
        // none at all.
        if (Object.keys(assets).length === 0) return;

        setRelease({
          version: (data.tag_name ?? "").replace(/^v/, "") || FALLBACK.version,
          published: data.published_at ?? null,
          assets,
          stale: false,
        });
      })
      .catch(() => {
        /* Rate limited, offline, or blocked. The fallback already stands. */
      });

    return () => abort.abort();
  }, []);

  return release;
}

/*
 * Detection reads the platform, never the browser. `userAgentData.platform` is
 * the one that is not a compatibility string; the userAgent is the fallback for
 * Firefox and Safari, which do not have it. A phone is deliberately "unknown" —
 * there is no build for it and pretending otherwise hands someone a 9 MB file
 * they cannot run.
 */
export function detectOs(): Os {
  if (typeof navigator === "undefined") return "unknown";

  const uaData = (navigator as Navigator & { userAgentData?: { platform?: string } })
    .userAgentData;
  const platform = (uaData?.platform || navigator.platform || "").toLowerCase();
  const ua = navigator.userAgent.toLowerCase();

  if (/android|iphone|ipad|ipod/.test(ua)) return "unknown";
  // iPadOS reports itself as a Mac; it has a touch point count and no build.
  if (/mac/.test(platform) && navigator.maxTouchPoints > 1) return "unknown";

  if (/win/.test(platform) || /windows/.test(ua)) return "windows";
  if (/mac|darwin/.test(platform) || /mac os x/.test(ua)) return "macos";
  if (/linux|x11|cros|bsd/.test(platform) || /linux|x11|cros|bsd/.test(ua)) return "linux";

  return "unknown";
}

export function formatSize(bytes?: number): string {
  if (!bytes) return "";
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** Drops the artifacts a release does not carry, keeping the rest typed. */
function present(list: (Download | null)[]): Download[] {
  return list.filter((a): a is Download => a !== null);
}

/** Matches an asset by shape rather than by exact name, since the version moves. */
function find(release: Release, test: (name: string) => boolean) {
  const name = Object.keys(release.assets).find(test);
  return name ? { name, ...release.assets[name] } : null;
}

export function platformFor(os: Os, release: Release): Platform {
  const exe = find(release, (n) => n.endsWith(".exe"));
  const deb = find(release, (n) => n.endsWith(".deb"));
  const rpm = find(release, (n) => n.endsWith(".rpm"));
  const tar = find(release, (n) => n.endsWith(".tar.gz"));

  switch (os) {
    case "windows":
      return {
        os,
        name: "Windows",
        primary: exe && {
          label: "Download for Windows",
          detail: exe.name,
          url: exe.url,
          size: exe.size,
        },
        alternatives: [],
        note: "Per-user, under %LOCALAPPDATA%\\Programs\\vex — no admin prompt. Adds a Start Menu entry, vex on your PATH, “Open with vex” on a folder, and an uninstaller.",
        prompt: ">",
        command: ".\\vex-setup.exe",
      };

    case "linux":
      return {
        os,
        name: "Linux",
        primary: deb && {
          label: "Download for Linux",
          detail: deb.name,
          url: deb.url,
          size: deb.size,
        },
        alternatives: present([
          rpm && { label: ".rpm — Fedora, RHEL, openSUSE", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: ".tar.gz — any distribution", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "Debian, Ubuntu and Mint. The tarball installs into ~/.local and needs no root at all.",
        prompt: "$",
        command: deb ? `sudo apt install ./${deb.name}` : "cargo build --release",
      };

    case "macos":
      return {
        os,
        name: "macOS",
        // Not an oversight and not a "coming soon": there is no macOS build,
        // and nobody has ever run vex there. Saying so is the honest button.
        primary: null,
        alternatives: [],
        note: "There is no macOS build, and nobody has ever run vex on macOS. It should compile — the code is portable — but nothing about the window, the mouse or the folder chooser has been verified there.",
        prompt: "$",
        command: `git clone ${REPO_URL} && cd Vex && cargo build --release`,
      };

    default:
      return {
        os: "unknown",
        name: "your platform",
        primary: null,
        alternatives: present([
          exe && { label: "Windows — vex-setup.exe", detail: exe.name, url: exe.url, size: exe.size },
          deb && { label: "Linux — .deb", detail: deb.name, url: deb.url, size: deb.size },
          rpm && { label: "Linux — .rpm", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: "Linux — .tar.gz", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "vex is a desktop editor for x86-64 Linux and Windows. Pick the artifact for the machine you will run it on.",
        prompt: "$",
        command: `git clone ${REPO_URL}`,
      };
  }
}
