const themeStorageKey = "iowb.landing.theme";
const darkThemeColor = "#0c1310";
const lightThemeColor = "#f4f7f5";
const root = document.documentElement;
const themeToggle = document.querySelector("[data-theme-toggle]");
const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
const article = document.querySelector(".docs-article");
const outline = document.getElementById("on-this-page");
const searchInput = document.getElementById("docs-search");
const searchResults = document.getElementById("docs-search-results");
const legacyDocsHashRoutes = Object.freeze({
  overview: "/docs/",
  "quick-start": "/docs/quick-start/",
  "remote-access": "/docs/remote-access/",
  agents: "/docs/agents-and-sessions/",
  workspace: "/docs/projects-files-git/",
  database: "/docs/database-workspace/",
  terminal: "/docs/terminal-and-mobile/",
  automation: "/docs/boards-and-tools/",
  security: "/docs/security-and-boundaries/",
  configuration: "/docs/configuration/",
  api: "/docs/api/",
});

function decodedHash() {
  const hash = window.location.hash.slice(1);
  try {
    return decodeURIComponent(hash).trim().toLowerCase();
  } catch {
    return hash.trim().toLowerCase();
  }
}

function isDocsRootPath(pathname) {
  const normalized = pathname.replace(/\/+$/, "") || "/";
  return normalized === "/docs" || normalized === "/docs.html" || normalized === "/docs/index.html";
}

function redirectLegacyDocsHash() {
  if (!isDocsRootPath(window.location.pathname)) return false;
  const destination = legacyDocsHashRoutes[decodedHash()];
  if (!destination) return false;

  window.location.replace(destination + window.location.search);
  return true;
}

redirectLegacyDocsHash();

function savedTheme() {
  try {
    const theme = window.localStorage.getItem(themeStorageKey);
    return theme === "light" || theme === "dark" ? theme : null;
  } catch {
    return null;
  }
}

function preferredTheme() {
  return systemTheme.matches ? "dark" : "light";
}

function applyTheme(theme) {
  const nextTheme = theme === "dark" ? "dark" : "light";
  root.dataset.theme = nextTheme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute(
    "content",
    nextTheme === "dark" ? darkThemeColor : lightThemeColor,
  );

  if (!themeToggle) return;
  const nextThemeLabel = nextTheme === "dark" ? "light" : "dark";
  themeToggle.setAttribute("aria-label", "Switch to " + nextThemeLabel + " theme");
  themeToggle.setAttribute("aria-pressed", String(nextTheme === "dark"));
  themeToggle.title = "Switch to " + nextThemeLabel + " theme";
}

applyTheme(savedTheme() ?? preferredTheme());

themeToggle?.addEventListener("click", () => {
  const nextTheme = root.dataset.theme === "dark" ? "light" : "dark";
  try {
    window.localStorage.setItem(themeStorageKey, nextTheme);
  } catch {
    // The selected theme still applies for this visit when storage is unavailable.
  }
  applyTheme(nextTheme);
});

function followSystemTheme(event) {
  if (!savedTheme()) applyTheme(event.matches ? "dark" : "light");
}

if (systemTheme.addEventListener) {
  systemTheme.addEventListener("change", followSystemTheme);
} else {
  systemTheme.addListener(followSystemTheme);
}

function slugify(value) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function normalizePath(pathname) {
  const clean = pathname.replace(/index\.html$/, "").replace(/\/+$/, "");
  return clean === "" ? "/" : clean + "/";
}

function installActivePageState() {
  const currentPath = normalizePath(window.location.pathname);
  document.querySelectorAll("[data-doc-link][href]").forEach((link) => {
    const linkPath = normalizePath(new URL(link.href, window.location.origin).pathname);
    const isActive = linkPath === currentPath;
    link.classList.toggle("is-active", isActive);
    if (isActive) {
      link.setAttribute("aria-current", "page");
    } else {
      link.removeAttribute("aria-current");
    }
  });
}

function headingsForOutline() {
  if (!article) return [];
  return [...article.querySelectorAll("h2, h3")].filter(
    (heading) => !heading.closest(".docs-grid, .docs-page-list, .docs-table-wrap, .docs-related"),
  );
}

function ensureHeadingIds(headings) {
  const usedIds = new Set([...document.querySelectorAll("[id]")].map((element) => element.id));
  headings.forEach((heading) => {
    if (heading.id) return;
    const base = slugify(heading.textContent.trim()) || "section";
    let id = base;
    let suffix = 2;
    while (usedIds.has(id)) {
      id = base + "-" + suffix;
      suffix += 1;
    }
    heading.id = id;
    usedIds.add(id);
  });
}

function buildOutline() {
  if (!outline) return [];
  const headings = headingsForOutline();
  ensureHeadingIds(headings);
  const fragment = document.createDocumentFragment();

  headings.forEach((heading) => {
    const link = document.createElement("a");
    link.href = "#" + heading.id;
    link.textContent = heading.textContent.trim();
    link.dataset.depth = heading.tagName === "H3" ? "3" : "2";
    fragment.appendChild(link);
  });

  if (!fragment.childNodes.length) {
    outline.parentElement?.setAttribute("hidden", "");
    return headings;
  }

  outline.replaceChildren(fragment);
  return headings;
}

function installBreadcrumbs() {
  if (!article || article.querySelector(".docs-breadcrumbs")) return;

  const title = article.querySelector("h1")?.textContent.trim() || "Documentation";
  const group = article.dataset.pageGroup || article.querySelector(".docs-eyebrow")?.textContent.trim();
  const breadcrumbs = document.createElement("nav");
  breadcrumbs.className = "docs-breadcrumbs";
  breadcrumbs.setAttribute("aria-label", "Breadcrumb");

  const home = document.createElement("a");
  home.href = "/landing";
  home.textContent = "io-workbench";

  const docs = document.createElement("a");
  docs.href = "/docs/";
  docs.textContent = "Docs";

  breadcrumbs.append(home, breadcrumbDivider(), docs);
  if (article.dataset.pageSlug && group) {
    const category = document.createElement("span");
    category.textContent = group;
    breadcrumbs.append(breadcrumbDivider(), category);
  }

  const current = document.createElement("span");
  current.setAttribute("aria-current", "page");
  current.textContent = article.dataset.pageSlug ? title : "Documentation";
  breadcrumbs.append(breadcrumbDivider(), current);
  article.prepend(breadcrumbs);
}

function breadcrumbDivider() {
  const divider = document.createElement("span");
  divider.setAttribute("aria-hidden", "true");
  divider.textContent = "/";
  return divider;
}

function installMobileOutline() {
  if (!article || !outline || article.querySelector(".docs-mobile-outline")) return;
  const links = [...outline.querySelectorAll("a")];
  if (!links.length) return;

  const details = document.createElement("details");
  details.className = "docs-mobile-outline";
  const summary = document.createElement("summary");
  summary.textContent = "Contents";
  const nav = document.createElement("nav");
  links.forEach((link) => nav.appendChild(link.cloneNode(true)));
  details.append(summary, nav);

  const lead = article.querySelector(".docs-lead");
  if (lead) {
    lead.after(details);
  } else {
    article.querySelector(".docs-heading")?.after(details);
  }

  details.querySelectorAll("a").forEach((link) => {
    link.addEventListener("click", () => details.removeAttribute("open"));
  });
}

function installHeadingSpy(headings) {
  const links = [...document.querySelectorAll(".docs-on-page a")];
  if (!links.length || !headings.length) return;

  const linksById = new Map(links.map((link) => [decodeURIComponent(link.hash.slice(1)), link]));
  function setActive(heading) {
    if (!heading) return;
    links.forEach((link) => link.classList.toggle("is-active", link === linksById.get(heading.id)));
  }

  const hashTarget = document.getElementById(decodeURIComponent(window.location.hash.slice(1)));
  setActive(hashTarget?.matches("h2, h3") ? hashTarget : headings[0]);
  if (!("IntersectionObserver" in window)) return;

  const observer = new IntersectionObserver(
    (entries) => {
      const visible = entries
        .filter((entry) => entry.isIntersecting)
        .sort((left, right) => left.boundingClientRect.top - right.boundingClientRect.top);
      if (visible[0]?.target) setActive(visible[0].target);
    },
    { rootMargin: "-90px 0px -70% 0px", threshold: 0.01 },
  );
  headings.forEach((heading) => observer.observe(heading));

  window.addEventListener("hashchange", () => {
    const target = document.getElementById(decodeURIComponent(window.location.hash.slice(1)));
    if (target?.matches("h2, h3")) setActive(target);
  });
}

function buildFallbackSearchPages() {
  return [...document.querySelectorAll("[data-doc-link]")].map((link) => ({
    title: link.dataset.title || link.textContent.trim(),
    category: link.dataset.category || "",
    summary: link.dataset.summary || "",
    keywords: link.dataset.keywords || "",
    headings: "",
    excerpt: "",
    href: link.getAttribute("href") || "/docs/",
  }));
}

function installSearch() {
  if (!searchInput || !searchResults) return;
  let pages = buildFallbackSearchPages();

  fetch("/docs/search-index.json", { cache: "no-store" })
    .then((response) => (response.ok ? response.json() : Promise.reject(new Error("search index unavailable"))))
    .then((index) => {
      if (!Array.isArray(index.pages) || !index.pages.length) return;
      pages = index.pages.map((page) => ({
        title: page.title || "",
        category: page.category || "",
        summary: page.summary || "",
        keywords: page.keywords || "",
        headings: Array.isArray(page.headings) ? page.headings.join(" ") : "",
        excerpt: page.excerpt || "",
        href: page.href || "/docs/",
      }));
    })
    .catch(() => {
      pages = buildFallbackSearchPages();
    });

  function clearResults() {
    searchResults.classList.remove("is-visible");
    searchResults.replaceChildren();
  }

  searchInput.addEventListener("input", () => {
    const query = searchInput.value.trim().toLowerCase();
    if (!query) {
      clearResults();
      return;
    }

    const matches = pages
      .map((page) => {
        const title = page.title.toLowerCase();
        const category = page.category.toLowerCase();
        const summary = page.summary.toLowerCase();
        const keywords = page.keywords.toLowerCase();
        const headings = page.headings.toLowerCase();
        const excerpt = page.excerpt.toLowerCase();
        const haystack = [title, category, summary, keywords, headings, excerpt].join(" ");
        let score = 0;
        if (title.includes(query)) score += 6;
        if (category.includes(query)) score += 3;
        if (headings.includes(query)) score += 3;
        if (keywords.includes(query)) score += 2;
        if (summary.includes(query)) score += 2;
        if (excerpt.includes(query)) score += 1;
        return { ...page, score: haystack.includes(query) ? score : 0 };
      })
      .filter((page) => page.score > 0)
      .sort((left, right) => right.score - left.score || left.title.localeCompare(right.title))
      .slice(0, 10);

    searchResults.replaceChildren();
    searchResults.classList.add("is-visible");
    if (!matches.length) {
      const empty = document.createElement("p");
      empty.className = "docs-search-empty";
      empty.textContent = "No documentation pages match that search.";
      searchResults.appendChild(empty);
      return;
    }

    matches.forEach((page) => {
      const link = document.createElement("a");
      link.href = page.href;
      const title = document.createElement("strong");
      title.textContent = page.title;
      const summary = document.createElement("small");
      summary.textContent = [page.category, page.summary].filter(Boolean).join(" — ");
      link.append(title, summary);
      link.addEventListener("click", clearResults);
      searchResults.appendChild(link);
    });
  });

  searchInput.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      searchInput.value = "";
      clearResults();
    }
  });
}

function installCopyButtons() {
  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      const value = button.dataset.copy;
      const card = button.closest(".docs-code-card");
      const status = card?.querySelector(".docs-copy-status");
      if (!value || !status) return;

      try {
        await navigator.clipboard.writeText(value);
        status.textContent = "Copied";
        button.textContent = "Copied";
      } catch {
        status.textContent = "Select and copy the command";
      }

      window.setTimeout(() => {
        status.textContent = "";
        button.textContent = "Copy";
      }, 1800);
    });
  });
}

installActivePageState();
installBreadcrumbs();
const headings = buildOutline();
installMobileOutline();
installHeadingSpy(headings);
installSearch();
installCopyButtons();
