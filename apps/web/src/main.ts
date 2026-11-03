import init, { WebSession } from "./generated/wasm/lapis_web.js";

type SharedDocument = {
  document_id: string | null;
  path: string | null;
  cursor_char: number;
  selection_start: number;
  selection_end: number;
  scroll_x: number;
  scroll_y: number;
  draft_content: string | null;
};

type SharedWorkspace = {
  workspace_id: string;
  open_documents: SharedDocument[];
  active_path: string | null;
  active_document_id: string | null;
};

type View = {
  show_panel_controls?: boolean;
  workspace?: SharedWorkspace | null;
  [key: string]: unknown;
};

type SessionState = {
  revision: number;
  view: View | null;
  pending: boolean;
  conflict: { revision: number; view: View | null } | null;
};

const root = document.querySelector<HTMLElement>("#app");
if (root === null) {
  throw new Error("Lapis Web root was not found");
}

root.innerHTML = `
  <section class="card connect" id="connect">
    <p class="eyebrow">LAPIS WEB</p>
    <h1>同じHostの作業を継続</h1>
    <p class="lede">Hostとweb-bridgeは同じPCで動かし、ブラウザーからは中継サーバー経由で接続します。</p>
    <form id="connect-form">
      <label>WebSocket URL<input id="url" placeholder="ws://127.0.0.1:8787/remote" autocomplete="off" /></label>
      <label>Workspace ID<input id="workspace" placeholder="workspace-1" autocomplete="off" /></label>
      <label>Pairing token<input id="token" placeholder="初回だけ入力" autocomplete="off" /></label>
      <button type="submit">接続</button>
    </form>
    <p class="status" id="status"><i aria-hidden="true"></i><span>未接続</span></p>
  </section>
  <section class="workbench" id="workbench" hidden>
    <header class="workbench-header">
      <div>
        <p class="eyebrow">SHARED WORKBENCH</p>
        <p class="revision" id="revision">revision 0</p>
      </div>
      <p class="status" id="live"><i aria-hidden="true"></i><span>接続済み</span></p>
    </header>
    <div class="conflict" id="conflict" hidden>
      <p>他のクライアントが先に更新しました。未確定の入力は保持しています。</p>
      <div class="actions">
        <button type="button" id="accept">Hostの状態を採用</button>
        <button type="button" id="retry">自分の変更を再送</button>
      </div>
    </div>
    <div class="documents" id="documents"></div>
    <button type="button" class="add" id="add">下書きを追加</button>
  </section>
`;

const form = element<HTMLFormElement>("connect-form");
const urlInput = element<HTMLInputElement>("url");
const workspaceInput = element<HTMLInputElement>("workspace");
const tokenInput = element<HTMLInputElement>("token");
const status = element<HTMLElement>("status");
const workbench = element<HTMLElement>("workbench");
const revision = element<HTMLElement>("revision");
const live = element<HTMLElement>("live");
const conflictPanel = element<HTMLElement>("conflict");
const documents = element<HTMLElement>("documents");
const acceptButton = element<HTMLButtonElement>("accept");
const retryButton = element<HTMLButtonElement>("retry");
const addButton = element<HTMLButtonElement>("add");

const STORAGE = {
  url: "lapis.web.url",
  workspace: "lapis.web.workspace",
  clientId: "lapis.web.clientId",
};

const params = new URLSearchParams(location.search);
urlInput.value = params.get("url") ?? localStorage.getItem(STORAGE.url) ?? "";
workspaceInput.value =
  params.get("workspace") ?? localStorage.getItem(STORAGE.workspace) ?? "";
tokenInput.value = params.get("token") ?? "";

let session: WebSession | null = null;
let state: SessionState = { revision: 0, view: null, pending: false, conflict: null };
let view: View = {};
let workspaceId = "";
let saving = false;
let queued = false;
let dirty = false;
let editVersion = 0;
let saveTimer: number | undefined;
// wasm-bindgenの&mut借用は同時に1つだけなので、セッション操作を直列化する。
let chain: Promise<unknown> = Promise.resolve();

function enqueue<T>(task: () => Promise<T>): Promise<T> {
  const run = chain.then(task, task);
  chain = run.catch(() => undefined);
  return run;
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void connect();
});

addButton.addEventListener("click", () => {
  const workspace = ensureWorkspace();
  workspace.open_documents.push({
    document_id: null,
    path: null,
    cursor_char: 0,
    selection_start: 0,
    selection_end: 0,
    scroll_x: 0,
    scroll_y: 0,
    draft_content: "",
  });
  editVersion += 1;
  dirty = true;
  renderDocuments();
  scheduleSave();
});

acceptButton.addEventListener("click", () => {
  const current = session;
  if (current === null) {
    return;
  }
  void (async () => {
    try {
      const next = JSON.parse(
        await enqueue(async () => current.acceptRemoteJson()),
      ) as SessionState;
      applyState(next);
      setStatus(status, "Hostの状態を採用しました", "ok");
    } catch (error) {
      setStatus(status, `採用できません: ${String(error)}`, "error");
    }
  })();
});

retryButton.addEventListener("click", () => {
  void retryLocal();
});

async function connect(): Promise<void> {
  const url = urlInput.value.trim();
  const workspace = workspaceInput.value.trim();
  if (url === "" || workspace === "") {
    setStatus(status, "URLとWorkspace IDを入力してください", "error");
    return;
  }
  try {
    setStatus(status, "接続中…", "busy");
    await init();
    const clientId = localStorage.getItem(STORAGE.clientId) ?? crypto.randomUUID();
    localStorage.setItem(STORAGE.clientId, clientId);
    const credentialKey = `${url}|${workspace}`;
    const credential = localStorage.getItem(credentialKey);
    session = await WebSession.connect(
      url,
      clientId,
      workspace,
      tokenInput.value.trim() || null,
      credential,
    );
    const fresh = session.takeCredential();
    if (fresh !== undefined && fresh !== null) {
      localStorage.setItem(credentialKey, fresh);
      tokenInput.value = "";
    }
    localStorage.setItem(STORAGE.url, url);
    localStorage.setItem(STORAGE.workspace, workspace);
    workspaceId = workspace;
    session.startEvents(() => {
      void refresh();
    });
    workbench.hidden = false;
    form.closest("section")?.classList.add("compact");
    applyState(JSON.parse(session.stateJson()) as SessionState);
    setStatus(status, "接続済み", "ok");
    setStatus(live, "接続済み", "ok");
  } catch (error) {
    setStatus(status, `接続できません: ${String(error)}`, "error");
  }
}

async function refresh(): Promise<void> {
  const current = session;
  if (current === null) {
    return;
  }
  try {
    const next = JSON.parse(
      await enqueue(async () => current.refreshJson()),
    ) as SessionState;
    applyState(next, dirty || saving);
  } catch (error) {
    setStatus(live, "Hostとの接続が終了しました。再接続してください。", "error");
    setStatus(status, `切断: ${String(error)}`, "error");
  }
}

async function retryLocal(): Promise<void> {
  const current = session;
  if (current === null) {
    return;
  }
  try {
    const next = JSON.parse(
      await enqueue(async () => current.retryLocalJson()),
    ) as SessionState;
    applyState(next);
    setStatus(status, "自分の変更を再送しました", "ok");
  } catch (error) {
    setStatus(status, `再送できません: ${String(error)}`, "error");
  }
}

function applyState(next: SessionState, keepInputs = false): void {
  state = next;
  revision.textContent = `revision ${next.revision}`;
  conflictPanel.hidden = next.conflict === null;
  if (!keepInputs && !dirty) {
    view = next.view ?? emptyView();
    ensureWorkspace();
    renderDocuments();
  }
}

function emptyView(): View {
  return {
    workspace: {
      workspace_id: workspaceId,
      open_documents: [],
      active_path: null,
      active_document_id: null,
    },
  };
}

function ensureWorkspace(): SharedWorkspace {
  if (view.workspace === undefined || view.workspace === null) {
    view.workspace = {
      workspace_id: workspaceId,
      open_documents: [],
      active_path: null,
      active_document_id: null,
    };
  }
  if (view.workspace.workspace_id !== workspaceId) {
    view.workspace.workspace_id = workspaceId;
  }
  return view.workspace;
}

function renderDocuments(): void {
  const workspace = ensureWorkspace();
  documents.replaceChildren(
    ...workspace.open_documents.map((document, index) => {
      const card = documentCard(document, index);
      return card;
    }),
  );
  if (workspace.open_documents.length === 0) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent =
      "このWorkspaceの共有状態にはまだ文書がありません。Desktopで文書を開くか、下書きを追加してください。";
    documents.append(empty);
  }
}

function documentCard(entry: SharedDocument, index: number): HTMLElement {
  const card = document.createElement("article");
  card.className = "document";
  const header = document.createElement("header");
  const title = document.createElement("span");
  title.className = "path";
  title.textContent = entry.path ?? "名称未設定の下書き";
  const cursor = document.createElement("span");
  cursor.className = "cursor";
  cursor.textContent = `cursor ${entry.cursor_char}`;
  header.append(title, cursor);
  const textarea = document.createElement("textarea");
  textarea.value = entry.draft_content ?? "";
  textarea.rows = 10;
  textarea.spellcheck = false;
  textarea.addEventListener("input", () => {
    const workspace = ensureWorkspace();
    workspace.open_documents[index].draft_content = textarea.value;
    editVersion += 1;
    dirty = true;
    scheduleSave();
  });
  card.append(header, textarea);
  if (state.conflict !== null && state.pending) {
    card.classList.add("conflicted");
  }
  return card;
}

function scheduleSave(): void {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    void save();
  }, 500);
}

async function save(): Promise<void> {
  const current = session;
  if (current === null) {
    return;
  }
  if (saving) {
    queued = true;
    return;
  }
  saving = true;
  const version = editVersion;
  try {
    const next = JSON.parse(
      await enqueue(async () => current.saveJson(JSON.stringify(view))),
    ) as SessionState;
    if (editVersion === version) {
      dirty = false;
      applyState(next);
      setStatus(
        status,
        next.conflict === null ? "保存しました" : "競合しています",
        next.conflict === null ? "ok" : "warn",
      );
    } else {
      applyState(next, true);
      setStatus(status, "続けて保存します", "busy");
    }
  } catch (error) {
    setStatus(status, `保存できません: ${String(error)}`, "error");
  } finally {
    saving = false;
    if (queued || (editVersion !== version && dirty)) {
      queued = false;
      void save();
    }
  }
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (found === null) {
    throw new Error(`#${id} was not found`);
  }
  return found as T;
}

function setStatus(target: HTMLElement, text: string, kind: "ok" | "error" | "busy" | "warn"): void {
  target.dataset.kind = kind;
  const label = target.querySelector("span");
  if (label !== null) {
    label.textContent = text;
  } else {
    target.textContent = text;
  }
}

const storedCredential =
  urlInput.value !== "" && workspaceInput.value !== ""
    ? localStorage.getItem(`${urlInput.value}|${workspaceInput.value}`)
    : null;
if (
  urlInput.value !== "" &&
  workspaceInput.value !== "" &&
  (tokenInput.value !== "" || storedCredential !== null)
) {
  void connect();
}
