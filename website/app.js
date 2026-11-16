// OpenFlow site: OS detection, latest release info, copy buttons, examples, hero demo.
(() => {
  "use strict";
  document.documentElement.classList.add("js");
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const REPO = "Vivek-C-Shah/openflow";

  /* ---------- OS detection ---------- */
  const ua = navigator.userAgent;
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  const mobile = /Android|iPhone|iPad|iPod/i.test(ua) || (navigator.userAgentData && navigator.userAgentData.mobile);
  const os = mobile ? null
    : /Win/i.test(platform) || /Windows/i.test(ua) ? "windows"
    : /Mac/i.test(platform) || /Mac OS/i.test(ua) ? "macos"
    : /Linux|X11|CrOS/i.test(platform + ua) ? "linux"
    : null;
  const names = { windows: "Windows", macos: "macOS", linux: "Linux" };

  if (os) {
    const label = $("[data-os-label]");
    if (label) label.textContent = `Download for ${names[os]}`;
  }
  if (mobile) $("[data-mobile-note]")?.removeAttribute("hidden");

  /* ---------- OS tabs ---------- */
  function showOs(name) {
    $$(".os-tabs [role=tab]").forEach((t) => t.setAttribute("aria-selected", String(t.dataset.os === name)));
    $$("[data-panel]").forEach((p) => { p.hidden = p.dataset.panel !== name; });
  }
  $$(".os-tabs [role=tab]").forEach((t) => t.addEventListener("click", () => showOs(t.dataset.os)));
  showOs(os || "windows");

  /* ---------- Latest release (label only; download links use /releases/latest so they never go stale) ---------- */
  const meta = $("[data-release]");
  fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } })
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((rel) => {
      const date = new Date(rel.published_at).toLocaleDateString(undefined, { month: "long", day: "numeric", year: "numeric" });
      meta.innerHTML = "";
      const a = document.createElement("a");
      a.href = rel.html_url;
      a.textContent = `Version ${rel.tag_name.replace(/^v/, "")}`;
      meta.append(a, `, released ${date}. Free and open source under the MIT license.`);
    })
    .catch(() => { /* keep the static text; links still work */ });

  /* ---------- Copy buttons ---------- */
  $$(".copy").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const text = btn.parentElement.querySelector("code").textContent.trim();
      const label = btn.querySelector("span");
      const icon = btn.querySelector("i");
      try {
        await navigator.clipboard.writeText(text);
        btn.classList.add("done");
        label.textContent = "Copied";
        icon.className = "ph ph-check";
      } catch {
        const range = document.createRange();
        range.selectNodeContents(btn.parentElement.querySelector("code"));
        getSelection().removeAllRanges();
        getSelection().addRange(range);
        label.textContent = "Press Ctrl+C";
      }
      clearTimeout(btn._t);
      btn._t = setTimeout(() => {
        btn.classList.remove("done");
        label.textContent = "Copy";
        icon.className = "ph ph-copy";
      }, 1800);
    });
  });

  /* ---------- Examples (real cleanups) ---------- */
  // Words wrapped in [brackets] are what OpenFlow removes.
  const EXAMPLES = [
    {
      said: "[um] so I think we should [uh] meet on tuesday [no wait] wednesday at [like] 3 pm and discuss the [the] budget",
      typed: "I think we should meet on Wednesday at 3 PM and discuss the budget.",
    },
    {
      said: "hi priya [um] just wanted to say thanks for the deck it looked great [uh] can we push the review to friday morning thanks vivek",
      typed: "Hi Priya,\n\nJust wanted to say thanks for the deck, it looked great. Can we push the review to Friday morning?\n\nThanks,\nVivek",
    },
    {
      said: "[um] kal ki meeting cancel ho gayi hai [so] hum [uh] thursday ko 4 baje call karte hain",
      typed: "Kal ki meeting cancel ho gayi hai, so hum Thursday ko 4 baje call karte hain.",
    },
  ];
  const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
  const marked = (s) => esc(s).replace(/\[([^\]]+)\]/g, '<span class="filler">$1</span>');
  // Split into words, remembering which ones sit inside [brackets].
  const tokens = (s) => {
    let inside = false;
    return s.split(" ").map((raw) => {
      if (raw.startsWith("[")) inside = true;
      const t = { w: raw.replace(/[[\]]/g, ""), filler: inside };
      if (raw.endsWith("]")) inside = false;
      return t;
    });
  };
  // "[so]" in the Hinglish example is kept by OpenFlow; show it unmarked.
  EXAMPLES[2].said = EXAMPLES[2].said.replace("[so]", "so");

  const compare = $(".compare");
  const saidEl = $("[data-ex-said]");
  const typedEl = $("[data-ex-typed]");
  function showExample(i, animate) {
    const ex = EXAMPLES[i];
    $$(".tabs [role=tab]").forEach((t) => t.setAttribute("aria-selected", String(+t.dataset.ex === i)));
    const apply = () => {
      saidEl.innerHTML = marked(ex.said);
      typedEl.textContent = ex.typed;
      compare.classList.remove("swap");
    };
    if (animate && !reduce) {
      compare.classList.add("swap");
      setTimeout(apply, 200);
    } else apply();
  }
  $$(".tabs [role=tab]").forEach((t) => t.addEventListener("click", () => showExample(+t.dataset.ex, true)));
  showExample(0, false);

  /* ---------- Hero demo: idle -> listening -> writing -> typed ---------- */
  const pill = $("[data-pill]");
  const said = $("[data-said]");
  const typed = $("[data-typed]");
  const cap = $("[data-cap]");
  const bars = $$(".bars i", pill);
  const DEMOS = [EXAMPLES[0], EXAMPLES[2]];
  const CAPS = {
    idle: "Idle: a thin bar at the bottom of your screen",
    listening: "Listening: double-tapped Caps Lock",
    writing: "Writing: cleaning up what you said",
    done: "Typed into whatever app you were in",
  };

  if (reduce) {
    pill.dataset.state = "idle";
    pill.parentElement.dataset.phase = "done";
    said.innerHTML = marked(DEMOS[0].said);
    typed.textContent = DEMOS[0].typed;
    cap.textContent = CAPS.done;
    return;
  }

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  let visible = true;
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; }).observe(pill.parentElement);

  let levelTimer = 0;
  function animateBars(on) {
    clearInterval(levelTimer);
    if (!on) return;
    let t = 0;
    levelTimer = setInterval(() => {
      t += 1;
      const loud = 0.45 + 0.55 * Math.abs(Math.sin(t / 3.1));
      bars.forEach((b, i) => {
        const shape = 1 - Math.abs(i - 4) / 6;
        const h = 4 + 26 * shape * loud * (0.55 + Math.random() * 0.45);
        b.style.height = `calc(${h.toFixed(1)}px * var(--s))`;
      });
    }, 90);
  }
  function setState(s) {
    pill.dataset.state = s === "done" ? "idle" : s;
    pill.parentElement.dataset.phase = s;
    cap.textContent = CAPS[s];
    if (s !== "listening") bars.forEach((b) => (b.style.height = ""));
    animateBars(s === "listening");
  }

  async function run() {
    for (let n = 0; ; n++) {
      while (!visible || document.hidden) await sleep(400);
      const ex = DEMOS[n % DEMOS.length];
      typed.textContent = "";
      said.innerHTML = "";
      setState("idle");
      await sleep(1300);

      setState("listening");
      said.innerHTML = tokens(ex.said)
        .map((t) => `<span class="w${t.filler ? " filler" : ""}">${esc(t.w)}</span>`)
        .join(" ");
      const spans = $$(".w", said);
      for (const s of spans) {
        s.classList.add("on");
        await sleep(150 + Math.random() * 110);
      }
      await sleep(500);

      setState("writing");
      await sleep(900);

      setState("done");
      for (const ch of ex.typed) {
        typed.textContent += ch;
        await sleep(ch === " " ? 12 : 18);
      }
      await sleep(3200);
    }
  }
  run();
})();
