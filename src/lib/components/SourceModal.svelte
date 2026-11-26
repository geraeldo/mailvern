<script lang="ts">
  import Icon from "./Icon.svelte";
  import { api } from "../api";
  import { exportMessageEml, toast } from "../state.svelte";

  let {
    messageId,
    subject,
    source,
    onclose,
  }: {
    messageId: number;
    subject: string;
    source: string;
    onclose: () => void;
  } = $props();

  async function copySource() {
    try {
      await api.copyText(source);
      toast("Copied original source");
    } catch (e) {
      toast(String(e));
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scrim"
  onmousedown={(e) => { if (e.target === e.currentTarget) onclose(); }}
  onkeydown={(e) => { if (e.key === "Escape") { e.preventDefault(); onclose(); } }}>
  <div class="sheet" role="dialog" aria-label="Original message source">
    <header>
      <div class="head-text">
        <h3>Original Source</h3>
        <span class="sub">{subject || "(no subject)"}</span>
      </div>
      <div class="actions">
        <button type="button" class="btn sm" onclick={copySource}><Icon name="copy" size={14} /> Copy</button>
        <button type="button" class="btn sm" onclick={() => exportMessageEml(messageId, subject)}><Icon name="download" size={14} /> Export .eml</button>
        <button type="button" class="icon-btn" title="Close  Esc" onclick={onclose}><Icon name="x" size={16} /></button>
      </div>
    </header>
    <pre class="scroll">{source}</pre>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 65;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.22);
    animation: fade 0.1s ease-out;
  }
  .sheet {
    width: min(820px, calc(100vw - 48px));
    height: min(680px, calc(100vh - 64px));
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--sheet-border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px 12px 20px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .head-text { min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  h3 { margin: 0; font-size: 15px; font-weight: 600; color: var(--text); }
  .sub { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .actions { display: flex; align-items: center; gap: 6px; flex: none; }
  pre {
    flex: 1;
    margin: 0;
    padding: 16px 20px;
    font: 12px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
    color: var(--text-2);
    background: var(--bg);
    white-space: pre-wrap;
    word-break: break-all;
    -webkit-user-select: text;
    user-select: text;
  }
</style>
