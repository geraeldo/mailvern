<script lang="ts">
  import { initials, colorFor } from "../format";
  import { senderIcon } from "../avatars.svelte";

  let {
    name,
    email,
    size = 32,
    lookup = true,
  }: {
    name: string;
    email: string;
    size?: number;
    /** False to keep initials without looking up a picture (e.g. for junk mail). */
    lookup?: boolean;
  } = $props();

  const icon = $derived(lookup ? senderIcon(email) : null);
  /** An image the web view couldn't draw: falls back to initials. */
  let broken = $state<string | null>(null);
  const shown = $derived(icon && icon.url !== broken ? icon : null);
</script>

<span
  class="avatar"
  class:logo={shown?.logo}
  class:plate={shown && !shown.logo}
  style:--size="{size}px"
  style:background={shown ? null : colorFor(email)}
  aria-hidden="true">
  {#if shown}
    <img src={shown.url} alt="" draggable="false" onerror={() => (broken = shown.url)} />
  {:else}
    {initials(name, email)}
  {/if}
</span>

<style>
  .avatar {
    width: var(--size);
    height: var(--size);
    flex: none;
    border-radius: 50%;
    overflow: hidden;
    display: grid;
    place-items: center;
    color: #fff;
    font-size: calc(var(--size) * 0.36);
    font-weight: 600;
    letter-spacing: 0.02em;
    line-height: 1;
    user-select: none;
    -webkit-user-select: none;
  }
  img { display: block; pointer-events: none; }
  /* BIMI logos are drawn to fill a circle. */
  .logo img { width: 100%; height: 100%; object-fit: cover; }
  /* Website icons sit smaller on a light plate, so transparent ones read in dark mode too. */
  .plate { background: #fff; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08); }
  .plate img { width: 62%; height: 62%; object-fit: contain; }
</style>
