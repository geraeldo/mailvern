<script lang="ts">
  import Icon from "./Icon.svelte";
  import { api, type CalendarInvite } from "../api";
  import { toast } from "../state.svelte";

  let { messageId, invite }: { messageId: number; invite: CalendarInvite } = $props();

  let status = $state<string | null>(null);
  let sending = $state<string | null>(null);

  const activeStatus = $derived(status ?? invite.myStatus ?? null);
  const cancelled = $derived(invite.method === "CANCEL");

  function fmtRange(): string {
    if (!invite.start) return invite.dtstartRaw || "Time not specified";
    const s = new Date(invite.start * 1000);
    const e = invite.end ? new Date(invite.end * 1000) : null;
    if (invite.allDay) {
      const df = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
      return `${df.format(s)} · All day`;
    }
    const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });
    const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
    if (e) {
      const sameDay = s.toDateString() === e.toDateString();
      return sameDay
        ? `${day.format(s)}, ${time.format(s)} – ${time.format(e)}`
        : `${day.format(s)}, ${time.format(s)} – ${day.format(e)}, ${time.format(e)}`;
    }
    return `${day.format(s)}, ${time.format(s)}`;
  }

  async function respond(next: "ACCEPTED" | "TENTATIVE" | "DECLINED") {
    if (sending) return;
    sending = next;
    try {
      const saved = await api.rsvpInvite(messageId, next);
      status = saved;
      const lbl = saved === "ACCEPTED" ? "Accepted" : saved === "TENTATIVE" ? "Marked maybe" : "Declined";
      toast(`${lbl} — reply sent to organizer`);
    } catch (err) {
      toast(String(err));
    } finally {
      sending = null;
    }
  }

  async function addToCalendar() {
    try {
      await api.openIcs(messageId);
    } catch (err) {
      toast(String(err));
    }
  }
</script>

<div class="cal" class:cancelled>
  <div class="cal-head">
    <span class="cal-icon"><Icon name="calendar" size={16} /></span>
    <div class="cal-main">
      <div class="cal-title-row">
        <span class="cal-title">{invite.summary}</span>
        {#if cancelled}
          <span class="cal-badge cancel">Cancelled</span>
        {:else if activeStatus && activeStatus !== "NEEDS-ACTION"}
          <span class="cal-badge" class:yes={activeStatus === "ACCEPTED"} class:no={activeStatus === "DECLINED"}>
            {activeStatus === "ACCEPTED" ? "Accepted" : activeStatus === "TENTATIVE" ? "Maybe" : "Declined"}
          </span>
        {/if}
      </div>
      <div class="cal-when">{fmtRange()}</div>
      {#if invite.location}
        <div class="cal-meta">Location: {invite.location}</div>
      {/if}
      {#if invite.organizer}
        <div class="cal-meta">Organizer: {invite.organizer.name || invite.organizer.email}</div>
      {/if}
      {#if invite.attendees.length}
        <div class="cal-meta">
          Guests: {invite.attendees.slice(0, 6).map((a) => a.name || a.email).join(", ")}{invite.attendees.length > 6 ? ` +${invite.attendees.length - 6} more` : ""}
        </div>
      {/if}
    </div>
  </div>
  <div class="cal-actions">
    {#if !cancelled && invite.method === "REQUEST"}
      <button
        type="button"
        class="btn sm"
        class:primary={activeStatus === "ACCEPTED"}
        disabled={!!sending}
        onclick={() => respond("ACCEPTED")}>
        {sending === "ACCEPTED" ? "Sending…" : "Accept"}
      </button>
      <button
        type="button"
        class="btn sm"
        class:active-opt={activeStatus === "TENTATIVE"}
        disabled={!!sending}
        onclick={() => respond("TENTATIVE")}>
        {sending === "TENTATIVE" ? "Sending…" : "Maybe"}
      </button>
      <button
        type="button"
        class="btn sm"
        class:active-opt={activeStatus === "DECLINED"}
        disabled={!!sending}
        onclick={() => respond("DECLINED")}>
        {sending === "DECLINED" ? "Sending…" : "Decline"}
      </button>
    {/if}
    <span class="grow"></span>
    <button type="button" class="link" onclick={addToCalendar}>Open in Calendar</button>
  </div>
</div>

<style>
  .cal {
    margin-bottom: 16px;
    padding: 13px 15px;
    border: 1px solid var(--line-strong);
    border-radius: 10px;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: 11px;
  }
  .cal.cancelled { opacity: 0.85; }
  .cal-head { display: flex; align-items: flex-start; gap: 11px; }
  .cal-icon {
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: var(--accent-soft);
    color: var(--accent);
    display: grid;
    place-items: center;
    margin-top: 1px;
  }
  .cal-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .cal-title-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .cal-title { font-size: 14px; font-weight: 600; color: var(--text); }
  .cal-when { font-size: 13px; color: var(--text-2); }
  .cal-meta { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cal-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 99px;
    background: var(--hover);
    color: var(--text-2);
  }
  .cal-badge.yes { background: color-mix(in srgb, var(--success) 16%, transparent); color: var(--success); }
  .cal-badge.no, .cal-badge.cancel { background: var(--danger-soft); color: var(--danger); }
  .cal-actions {
    display: flex;
    align-items: center;
    gap: 7px;
    padding-top: 9px;
    border-top: 1px solid var(--line);
  }
  .active-opt { border-color: var(--accent); color: var(--accent); }
  .grow { flex: 1; }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .link:hover { text-decoration: underline; }
</style>
