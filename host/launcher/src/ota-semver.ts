export type SemVer = {
  major: number;
  minor: number;
  patch: number;
  prerelease: string | null;
};

/** Strip a leading `v` / `V` from a tag or version string. */
export function stripV(input: string): string {
  return input.trim().replace(/^v/i, '');
}

/**
 * Parse `vMAJOR.MINOR.PATCH` or `MAJOR.MINOR.PATCH` with optional prerelease.
 * Extra build metadata (`+…`) is ignored. Returns null if not a semver-like tag.
 */
export function parseSemver(input: string): SemVer | null {
  const s = stripV(input);
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(s);
  if (!m) return null;
  return {
    major: Number(m[1]),
    minor: Number(m[2]),
    patch: Number(m[3]),
    prerelease: m[4] ?? null,
  };
}

/** Negative if a < b, 0 if equal, positive if a > b (semver precedence). */
export function compareSemver(a: SemVer, b: SemVer): number {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  if (a.patch !== b.patch) return a.patch - b.patch;
  if (a.prerelease === null && b.prerelease !== null) return 1;
  if (a.prerelease !== null && b.prerelease === null) return -1;
  if (a.prerelease !== null && b.prerelease !== null) {
    if (a.prerelease < b.prerelease) return -1;
    if (a.prerelease > b.prerelease) return 1;
  }
  return 0;
}

/** True when `candidate` is a strictly higher semver than `current`. */
export function isNewerVersion(current: string, candidate: string): boolean {
  const a = parseSemver(current);
  const b = parseSemver(candidate);
  if (!a || !b) return false;
  return compareSemver(b, a) > 0;
}
