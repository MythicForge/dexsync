import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface LastSync { at: number; ok: boolean; message: string; written: number; devices: string[] }
interface SyncStatus {
  canConnect: boolean;
  clientId: string | null;
  connected: boolean;
  login: string | null;
  repo: string | null;
  backups: boolean;
  device: string;
  newRepoUrl: string;
  installUrl: string | null;
  last: LastSync | null;
}
interface ConnectCode { userCode: string; verificationUri: string }

type Toast = (text: string, kind?: "info" | "error") => void;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

let status: SyncStatus | null = null;
let pending: ConnectCode | null = null;
let repos: string[] | null = null;
let toast: Toast = () => {};

const openGitHub = (url: string) => invoke("open_github", { url }).catch((e) => toast(String(e), "error"));

function ago(secs: number): string {
  const d = Math.max(0, Math.round(Date.now() / 1000 - secs));
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  return new Date(secs * 1000).toLocaleString();
}

export async function refreshSync() {
  try {
    status = await invoke<SyncStatus>("sync_status");
  } catch (e) {
    toast(String(e), "error");
    return;
  }
  render();
}

function render() {
  const s = status!;

  // Account
  let account: string;
  if (s.connected) {
    account = `<div class="add-row"><span class="ok-dot"></span><span>Connected${s.login ? ` as <b>@${esc(s.login)}</b>` : ""}</span>
      <button class="ghost" id="disconnect">Disconnect</button></div>`;
  } else if (pending) {
    account = `<div class="code-box">
        <p>On GitHub, enter this code:</p>
        <div class="code">${esc(pending.userCode)}</div>
        <p class="muted">Waiting for you to approve… The page should have opened in your browser.</p>
        <div class="add-row"><button id="reopen">Open GitHub again</button><button class="ghost" id="cancel-connect">Cancel</button></div>
      </div>`;
  } else if (s.canConnect) {
    account = `<button class="primary" id="connect">Connect GitHub</button>`;
  } else {
    account = `<p class="muted">The public DexSync GitHub App isn't set up in this build yet. Use a personal access token under
      <b>Options → Advanced</b> for now.</p>`;
  }
  $("account").innerHTML = account;

  // Repo
  $("repo-panel").hidden = !s.connected;
  if (s.connected) {
    if (s.repo) {
      $("repo").innerHTML = `<div class="add-row"><code class="path">${esc(s.repo)}</code>
        <button class="ghost" data-open="https://github.com/${esc(s.repo)}">View on GitHub</button>
        <button class="ghost" id="change-repo">Change</button></div>`;
    } else {
      const picks = repos?.length
        ? `<ul class="rows">${repos.map((r) => `<li><code class="path">${esc(r)}</code><button data-pick="${esc(r)}">Use this repo</button></li>`).join("")}</ul>`
        : repos
          ? `<p class="muted">No repos have DexSync installed yet.</p>`
          : "";
      $("repo").innerHTML = `<ol class="steps">
          <li>Create a private repo for your data. <button data-open="${esc(s.newRepoUrl)}">Create repo on GitHub</button></li>
          ${s.installUrl ? `<li>Install DexSync on that repo only. <button data-open="${esc(s.installUrl)}">Install DexSync</button></li>` : ""}
          <li>Pick it here. <button id="find-repos">Find my repos</button></li>
        </ol>
        ${picks}
        <div class="add-row"><input id="repo-input" type="text" placeholder="or type owner/name" /><button id="save-repo">Use</button></div>`;
    }
  }

  // Status
  $("status-panel").hidden = !(s.connected && s.repo);
  const last = s.last;
  $("sync-status").innerHTML = last
    ? `<span class="${last.ok ? "ok" : "warn"}">${last.ok ? "✓" : "!"} ${esc(last.message)}</span>
       <span class="muted">${ago(last.at)}${last.ok ? ` · ${last.written} file(s) updated · devices: ${esc(last.devices.join(", "))}` : ""}</span>`
    : `<span class="muted">Not synced yet this session.</span>`;

  // Options (don't clobber what the user is typing)
  const backups = $<HTMLInputElement>("opt-backups");
  const deviceInput = $<HTMLInputElement>("opt-device");
  const clientInput = $<HTMLInputElement>("opt-client");
  if (document.activeElement !== deviceInput) deviceInput.placeholder = s.device;
  if (document.activeElement !== clientInput && !clientInput.value) clientInput.value = s.clientId ?? "";
  backups.checked = s.backups;
}

async function connect() {
  try {
    pending = await invoke<ConnectCode>("sync_connect");
  } catch (e) {
    toast(String(e), "error");
  }
  render();
}

export async function initSync(t: Toast) {
  toast = t;

  $("view-sync").addEventListener("click", async (e) => {
    const el = e.target as HTMLElement;
    if (el.dataset.open) return openGitHub(el.dataset.open);
    if (el.dataset.pick) {
      await invoke("sync_set_repo", { repo: el.dataset.pick }).catch((err) => toast(String(err), "error"));
      return refreshSync();
    }
    switch (el.id) {
      case "connect":
        return connect();
      case "reopen":
        return pending && openGitHub(pending.verificationUri);
      case "cancel-connect":
        await invoke("sync_cancel_connect");
        pending = null;
        return render();
      case "disconnect":
        await invoke("sync_disconnect").catch((err) => toast(String(err), "error"));
        repos = null;
        return refreshSync();
      case "find-repos":
        try {
          repos = await invoke<string[]>("sync_repos");
        } catch (err) {
          toast(String(err), "error");
          repos = [];
        }
        return render();
      case "save-repo":
      case "change-repo": {
        const repo = el.id === "change-repo" ? null : $<HTMLInputElement>("repo-input").value;
        try {
          await invoke("sync_set_repo", { repo });
        } catch (err) {
          toast(String(err), "error");
        }
        return refreshSync();
      }
      case "sync-now":
        toast("Syncing…");
        return invoke("sync_now");
      case "save-token": {
        const input = $<HTMLInputElement>("opt-token");
        try {
          const login = await invoke<string>("sync_set_token", { token: input.value });
          input.value = "";
          toast(`Connected as @${login}`);
        } catch (err) {
          toast(String(err), "error");
        }
        return refreshSync();
      }
      case "save-options":
        try {
          await invoke("sync_set_options", {
            backups: $<HTMLInputElement>("opt-backups").checked,
            clientId: $<HTMLInputElement>("opt-client").value || null,
            device: $<HTMLInputElement>("opt-device").value || null,
          });
          toast("Options saved");
        } catch (err) {
          toast(String(err), "error");
        }
        return refreshSync();
    }
  });

  await listen<{ ok: boolean; login?: string; error?: string }>("sync-auth", (e) => {
    pending = null;
    if (e.payload.ok) toast(`Connected as @${e.payload.login}`);
    else toast(e.payload.error ?? "Couldn't connect", "error");
    refreshSync();
  });
  await listen<LastSync>("sync-done", (e) => {
    if (!e.payload.ok) toast(`Sync failed: ${e.payload.message}`, "error");
    refreshSync();
  });

  await refreshSync();
}
