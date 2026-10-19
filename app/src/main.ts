import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { initSync, refreshSync } from "./sync";

// ── Types mirroring the Rust commands ───────────────────────────────────

interface Dex { owned: number[]; seen: number[]; ownedCount: number; seenCount: number }
interface SaveSummary { device: string; file: string; game: string; ownedCount: number; seenCount: number }
interface Trainer { name: string; tid: number; sid: number; games: string[]; dex: Dex; saves: SaveSummary[] }
interface Collection { schema: number; generated: number; overall: Dex; trainers: Trainer[] }
interface Source { id: string; preset: string; path: string; exists: boolean }
interface Suggestion { preset: string; path: string; label: string; exists: boolean; configured: boolean }
interface Overview { sources: Source[]; suggestions: Suggestion[]; sprites: string | null; collection: Collection; device: string }
interface Species { id: number; name: string; generation: number; types: string[] }
interface Change { source: string; file: string; game: string; trainer: string; first: boolean; newOwned: number[]; newSeen: number[] }

type Status = "all" | "owned" | "seen" | "missing";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const pad = (n: number) => String(n).padStart(4, "0");
const ROMAN = ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX"];
const PRESET_NAMES: Record<string, string> = { retroarch: "RetroArch", mgba: "mGBA", azahar: "Azahar", custom: "Folder" };

let species: Species[] = [];
let data: Overview | null = null;
const ui = { trainer: "all", status: "all" as Status, gen: 0, search: "" };

// ── Data ────────────────────────────────────────────────────────────────

async function refresh() {
  try {
    data = await invoke<Overview>("overview");
  } catch (e) {
    toast(String(e), "error");
    return;
  }
  renderTotals();
  renderTrainerSelect();
  renderGrid();
  renderSources();
}

function currentDex(): Dex | null {
  if (!data) return null;
  if (ui.trainer === "all") return data.collection.overall;
  return data.collection.trainers.find((t) => trainerKey(t) === ui.trainer)?.dex ?? null;
}

const trainerKey = (t: Trainer) => `${t.name}/${t.tid}/${t.sid}`;

// ── Header ──────────────────────────────────────────────────────────────

function renderTotals() {
  const d = data!.collection.overall;
  $("totals").innerHTML = `
    <span><b>${d.ownedCount}</b> owned</span>
    <span><b>${d.seenCount}</b> seen</span>
    <span class="muted">of ${species.length}</span>`;
}

// ── Pokédex view ────────────────────────────────────────────────────────

function renderTrainerSelect() {
  const sel = $<HTMLSelectElement>("trainer");
  const trainers = data!.collection.trainers;
  sel.innerHTML =
    `<option value="all">All saves</option>` +
    trainers
      .map((t) => `<option value="${esc(trainerKey(t))}">${esc(t.name)} · ${esc(t.games.join(", "))} · TID ${String(t.tid).padStart(5, "0")}</option>`)
      .join("");
  if (![...sel.options].some((o) => o.value === ui.trainer)) ui.trainer = "all";
  sel.value = ui.trainer;
}

function renderFilters() {
  const statuses: [Status, string][] = [["all", "All"], ["owned", "Owned"], ["seen", "Seen only"], ["missing", "Missing"]];
  $("status").innerHTML = statuses
    .map(([v, label]) => `<button data-status="${v}" class="${ui.status === v ? "on" : ""}">${label}</button>`)
    .join("");
  const gens = [...new Set(species.map((s) => s.generation))].sort((a, b) => a - b);
  $("gens").innerHTML =
    `<button data-gen="0" class="${ui.gen === 0 ? "on" : ""}">All gens</button>` +
    gens.map((g) => `<button data-gen="${g}" class="${ui.gen === g ? "on" : ""}">${ROMAN[g] ?? g}</button>`).join("");
}

function spriteUrl(id: number): string | null {
  if (!data?.sprites) return null;
  return convertFileSrc(`${data.sprites.replace(/[\\/]+$/, "")}/${id}.png`);
}

function renderGrid() {
  const dex = currentDex();
  const owned = new Set(dex?.owned ?? []);
  const seen = new Set(dex?.seen ?? []);
  const q = ui.search.trim().toLowerCase();

  const list = species.filter((s) => {
    if (ui.gen && s.generation !== ui.gen) return false;
    const st = owned.has(s.id) ? "owned" : seen.has(s.id) ? "seen" : "missing";
    if (ui.status !== "all" && ui.status !== st) return false;
    return !q || s.name.toLowerCase().includes(q) || String(s.id) === q.replace(/^#0*/, "");
  });

  $("sprite-hint").hidden = !!data?.sprites;
  $("grid").innerHTML = list
    .map((s) => {
      const st = owned.has(s.id) ? "owned" : seen.has(s.id) ? "seen" : "missing";
      const src = spriteUrl(s.id);
      const art = src
        ? `<img loading="lazy" src="${src}" alt="">`
        : `<span class="num">${s.id}</span>`;
      const name = st === "missing" ? "???" : esc(s.name);
      const label = st === "owned" ? "Owned" : st === "seen" ? "Seen" : "Not seen";
      return `<div class="card ${st}" title="#${pad(s.id)} ${esc(s.name)} — ${label}">
        <div class="art">${art}</div>
        <div class="meta"><span class="no">#${pad(s.id)}</span><span class="name">${name}</span></div>
      </div>`;
    })
    .join("");

  const empty = $("empty");
  empty.hidden = list.length > 0;
  empty.textContent = !data?.collection.trainers.length
    ? "No saves yet. Add your emulator folders in Sources, then save in-game."
    : "Nothing matches these filters.";
}

// ── Sources view ────────────────────────────────────────────────────────

function renderSources() {
  const d = data!;
  $("sources").innerHTML =
    d.sources
      .map(
        (s) => `<li>
          <span class="tag">${esc(PRESET_NAMES[s.preset] ?? s.preset)}</span>
          <code class="path">${esc(s.path)}</code>
          ${s.exists ? "" : `<span class="warn">folder missing</span>`}
          <button class="ghost" data-remove="${esc(s.id)}">Remove</button>
        </li>`,
      )
      .join("") || `<li class="muted">None yet. Add a detected folder below, or browse to one.</li>`;

  $("suggestions").innerHTML =
    d.suggestions
      .map(
        (s, i) => `<li>
          <span class="tag">${esc(PRESET_NAMES[s.preset] ?? s.preset)}</span>
          <div class="grow"><div>${esc(s.label)}</div><code class="path">${esc(s.path)}</code></div>
          ${s.exists ? "" : `<span class="muted">no saves yet</span>`}
          ${s.configured ? `<span class="ok">added</span>` : `<button data-suggest="${i}">Add</button>`}
        </li>`,
      )
      .join("") || `<li class="muted">No emulator folders detected.</li>`;

  $("sprites-path").textContent = d.sprites ?? "Not set";

  const saves = d.collection.trainers.flatMap((t) => t.saves.map((s) => ({ t, s })));
  $("saves").innerHTML =
    saves
      .map(
        ({ t, s }) => `<li>
          <span class="tag">${esc(s.game)}</span>
          <div class="grow"><div>${esc(t.name)} · TID ${String(t.tid).padStart(5, "0")} · ${esc(s.device)}</div><code class="path">${esc(s.file)}</code></div>
          <span><b>${s.ownedCount}</b> owned · <b>${s.seenCount}</b> seen</span>
        </li>`,
      )
      .join("") || `<li class="muted">No saves found yet.</li>`;
}

async function addSource(preset: string, path: string) {
  try {
    await invoke("add_source", { preset, path });
    toast(`Watching ${path}`);
  } catch (e) {
    toast(String(e), "error");
  }
  await refresh();
}

// ── Toasts ──────────────────────────────────────────────────────────────

function toast(text: string, kind: "info" | "error" = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = text;
  $("toasts").append(el);
  setTimeout(() => el.remove(), kind === "error" ? 8000 : 5000);
}

const nameOf = (id: number) => species[id - 1]?.name ?? `#${id}`;

// ── Wiring ──────────────────────────────────────────────────────────────

function showView(view: string) {
  document.querySelectorAll<HTMLButtonElement>(".tabs button").forEach((b) => b.classList.toggle("active", b.dataset.view === view));
  for (const v of ["dex", "sources", "sync"]) $(`view-${v}`).hidden = view !== v;
  if (view === "sync") refreshSync();
}

function wire() {
  document.querySelectorAll<HTMLButtonElement>(".tabs button").forEach((b) => (b.onclick = () => showView(b.dataset.view!)));
  document.querySelector<HTMLButtonElement>("[data-goto]")!.onclick = () => showView("sources");

  $<HTMLSelectElement>("trainer").onchange = (e) => {
    ui.trainer = (e.target as HTMLSelectElement).value;
    renderGrid();
  };
  $("status").onclick = (e) => {
    const v = (e.target as HTMLElement).dataset.status as Status | undefined;
    if (!v) return;
    ui.status = v;
    renderFilters();
    renderGrid();
  };
  $("gens").onclick = (e) => {
    const v = (e.target as HTMLElement).dataset.gen;
    if (v === undefined) return;
    ui.gen = Number(v);
    renderFilters();
    renderGrid();
  };
  // Missing sprite file: fall back to the number (inline handlers are blocked by the CSP).
  $("grid").addEventListener(
    "error",
    (e) => {
      const img = e.target as HTMLElement;
      if (img.tagName !== "IMG") return;
      const span = document.createElement("span");
      span.className = "num";
      span.textContent = img.closest(".card")?.querySelector(".no")?.textContent?.replace(/^#0*/, "") ?? "?";
      img.replaceWith(span);
    },
    true,
  );
  $<HTMLInputElement>("search").oninput = (e) => {
    ui.search = (e.target as HTMLInputElement).value;
    renderGrid();
  };

  $("sources").onclick = async (e) => {
    const id = (e.target as HTMLElement).dataset.remove;
    if (!id) return;
    try {
      await invoke("remove_source", { id });
    } catch (err) {
      toast(String(err), "error");
    }
    await refresh();
  };
  $("suggestions").onclick = async (e) => {
    const i = (e.target as HTMLElement).dataset.suggest;
    const s = i !== undefined ? data?.suggestions[Number(i)] : undefined;
    if (s) await addSource(s.preset, s.path);
  };
  $("add-browse").onclick = async () => {
    const path = await open({ directory: true, title: "Choose a save folder" });
    if (typeof path === "string") await addSource($<HTMLSelectElement>("add-preset").value, path);
  };
  $("sprites-browse").onclick = async () => {
    const path = await open({ directory: true, title: "Choose your sprites folder" });
    if (typeof path !== "string") return;
    await invoke("set_sprites", { path }).catch((e) => toast(String(e), "error"));
    await refresh();
  };
  $("sprites-clear").onclick = async () => {
    await invoke("set_sprites", { path: null }).catch((e) => toast(String(e), "error"));
    await refresh();
  };
}

async function main() {
  wire();
  species = await invoke<Species[]>("species_list");
  renderFilters();
  await refresh();
  if (!data?.sources.length) showView("sources");

  await listen<Change>("dex-changed", (e) => {
    const c = e.payload;
    if (c.first) toast(`Found ${c.trainer}'s ${c.game} save`);
    else if (c.newOwned.length) toast(`${c.game}: caught ${c.newOwned.map(nameOf).join(", ")}!`);
    else if (c.newSeen.length) toast(`${c.game}: saw ${c.newSeen.map(nameOf).join(", ")}`);
    refresh();
  });
  await listen<string>("watch-error", (e) => toast(e.payload, "error"));
  await initSync(toast);
}

main();
