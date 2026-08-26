(() => {
  "use strict";

  const root = document.documentElement;
  const body = document.body;
  const themeButton = document.querySelector("[data-theme-toggle]");
  const menuButton = document.querySelector("[data-menu-toggle]");
  const search = document.querySelector("[data-doc-search]");
  const documentBody = document.querySelector("[data-document]");
  const outline = document.querySelector("[data-outline]");

  const systemTheme = () =>
    window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";

  const applyTheme = (theme) => {
    root.dataset.theme = theme;
    if (themeButton) {
      themeButton.textContent = theme === "dark" ? "☀" : "☾";
      themeButton.setAttribute(
        "aria-label",
        theme === "dark" ? "Use light theme" : "Use dark theme",
      );
    }
  };

  let savedTheme = null;
  try {
    savedTheme = localStorage.getItem("glow-theme");
  } catch (_) {
    // Storage can be unavailable in hardened/private browser contexts.
  }
  applyTheme(savedTheme === "dark" || savedTheme === "light" ? savedTheme : systemTheme());

  themeButton?.addEventListener("click", () => {
    const next = root.dataset.theme === "dark" ? "light" : "dark";
    applyTheme(next);
    try {
      localStorage.setItem("glow-theme", next);
    } catch (_) {
      // The selected theme still applies for this page load.
    }
  });

  menuButton?.addEventListener("click", () => {
    const open = body.dataset.navOpen !== "true";
    body.dataset.navOpen = String(open);
    menuButton.setAttribute("aria-expanded", String(open));
  });

  document.querySelectorAll(".doc-link").forEach((link) => {
    link.addEventListener("click", () => {
      body.dataset.navOpen = "false";
      menuButton?.setAttribute("aria-expanded", "false");
    });
  });

  search?.addEventListener("input", () => {
    const query = search.value.trim().toLocaleLowerCase();
    document.querySelectorAll(".doc-link").forEach((link) => {
      link.hidden = query.length > 0 && !link.dataset.search.includes(query);
    });
  });

  const uniqueSlug = (() => {
    const seen = new Map();
    return (text) => {
      const base =
        text
          .normalize("NFKD")
          .toLocaleLowerCase()
          .replace(/[^\p{Letter}\p{Number}]+/gu, "-")
          .replace(/^-+|-+$/g, "") || "section";
      const count = seen.get(base) || 0;
      seen.set(base, count + 1);
      return count === 0 ? base : `${base}-${count + 1}`;
    };
  })();

  if (documentBody && outline) {
    const headings = [...documentBody.querySelectorAll("h2, h3, h4")];
    headings.forEach((heading) => {
      if (!heading.id) heading.id = uniqueSlug(heading.textContent || "section");
      const link = document.createElement("a");
      link.href = `#${encodeURIComponent(heading.id)}`;
      link.textContent = heading.textContent;
      link.dataset.level = heading.tagName.slice(1);
      outline.appendChild(link);
    });

    if (headings.length === 0) {
      outline.closest(".outline-panel")?.setAttribute("hidden", "");
    } else if ("IntersectionObserver" in window) {
      const linksById = new Map(
        [...outline.querySelectorAll("a")].map((link) => [
          decodeURIComponent(link.hash.slice(1)),
          link,
        ]),
      );
      const observer = new IntersectionObserver(
        (entries) => {
          const visible = entries.find((entry) => entry.isIntersecting);
          if (!visible) return;
          linksById.forEach((link) => link.classList.remove("is-active"));
          linksById.get(visible.target.id)?.classList.add("is-active");
        },
        { rootMargin: "-12% 0px -72%", threshold: 0 },
      );
      headings.forEach((heading) => observer.observe(heading));
    }
  }

  // The server rescans on its own interval. A tiny status response lets an open
  // browser refresh after the index or a file changes without a websocket.
  let knownRevision = Number(body.dataset.revision || "0");
  window.setInterval(async () => {
    if (document.visibilityState !== "visible") return;
    try {
      const response = await fetch("/api/status", {
        cache: "no-store",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) return;
      const status = await response.json();
      if (knownRevision > 0 && status.revision > knownRevision) {
        window.location.reload();
      }
      knownRevision = status.revision;
    } catch (_) {
      // A transient local/tunnel failure should not disturb reading.
    }
  }, 1500);
})();
