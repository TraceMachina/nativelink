export interface Contributor {
  login: string;
  avatar_url: string;
  html_url: string;
  contributions: number;
  type?: string;
}

interface ContributorsResult {
  contributors: Contributor[];
  total: number;
  fromCache: boolean;
}

const FALLBACK: Contributor[] = [
  {
    login: "aaronmondal",
    avatar_url: "https://github.com/aaronmondal.png?size=80",
    html_url: "https://github.com/aaronmondal",
    contributions: 0,
  },
  {
    login: "allada",
    avatar_url: "https://github.com/allada.png?size=80",
    html_url: "https://github.com/allada",
    contributions: 0,
  },
  {
    login: "blaise-d",
    avatar_url: "https://github.com/blaise-d.png?size=80",
    html_url: "https://github.com/blaise-d",
    contributions: 0,
  },
];

export async function getContributors(
  repo = "TraceMachina/nativelink",
): Promise<ContributorsResult> {
  const url = `https://api.github.com/repos/${repo}/contributors?per_page=30`;
  try {
    const res = await fetch(url, {
      headers: {
        Accept: "application/vnd.github+json",
        ...(process.env.GITHUB_TOKEN
          ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
          : {}),
      },
      next: { revalidate: 60 * 60 * 6 },
    });

    if (!res.ok) {
      return { contributors: FALLBACK, total: FALLBACK.length, fromCache: true };
    }

    const json = (await res.json()) as Contributor[];
    const filtered = json.filter((c) => c.type !== "Bot");
    return {
      contributors: filtered,
      total: filtered.length,
      fromCache: false,
    };
  } catch {
    return { contributors: FALLBACK, total: FALLBACK.length, fromCache: true };
  }
}

export interface RepoStats {
  stars: number;
  forks: number;
  openIssues: number;
  fromCache: boolean;
}

const FALLBACK_STATS: RepoStats = {
  stars: 4200,
  forks: 280,
  openIssues: 92,
  fromCache: true,
};

export async function getRepoStats(repo = "TraceMachina/nativelink"): Promise<RepoStats> {
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}`, {
      headers: {
        Accept: "application/vnd.github+json",
        ...(process.env.GITHUB_TOKEN
          ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
          : {}),
      },
      next: { revalidate: 60 * 60 * 6 },
    });
    if (!res.ok) return FALLBACK_STATS;
    const json = (await res.json()) as {
      stargazers_count: number;
      forks_count: number;
      open_issues_count: number;
    };
    return {
      stars: json.stargazers_count,
      forks: json.forks_count,
      openIssues: json.open_issues_count,
      fromCache: false,
    };
  } catch {
    return FALLBACK_STATS;
  }
}
