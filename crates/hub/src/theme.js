  // The theme: Auto (the system's), Light, Dark; remembered in this browser only.
  {
    const button = document.querySelector("[data-theme-btn]");
    const NAMES = { auto: "Auto", light: "Light", dark: "Dark" };
    const read = () => { try { return localStorage.getItem("mc-theme") || "auto"; } catch { return "auto"; } };
    const show = (t) => { button.textContent = NAMES[t]; button.setAttribute("aria-label", `Theme: ${NAMES[t]}`); };
    show(read());
    button.addEventListener("click", () => {
      const next = { auto: "light", light: "dark", dark: "auto" }[read()] || "auto";
      try { if (next === "auto") localStorage.removeItem("mc-theme"); else localStorage.setItem("mc-theme", next); } catch { /* not kept */ }
      if (next === "auto") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = next;
      show(next);
    });
  }
