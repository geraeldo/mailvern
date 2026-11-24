<script lang="ts">
  let {
    checked = $bindable(false),
    disabled = false,
    label = undefined,
    onchange,
  }: {
    checked?: boolean;
    disabled?: boolean;
    label?: string;
    onchange?: (checked: boolean, input: HTMLInputElement) => void;
  } = $props();
</script>

<input
  class="switch"
  type="checkbox"
  role="switch"
  aria-label={label}
  {checked}
  {disabled}
  onchange={(e) => {
    const next = e.currentTarget.checked;
    checked = next;
    onchange?.(next, e.currentTarget);
  }} />

<style>
  .switch {
    appearance: none;
    flex: none;
    position: relative;
    width: 34px;
    height: 20px;
    margin: 0;
    border: 0;
    border-radius: 10px;
    background: var(--line-strong);
    transition: background 0.15s ease;
    cursor: default;
  }
  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.22);
    transition: transform 0.15s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .switch:hover:not(:disabled) { background: color-mix(in srgb, var(--line-strong) 84%, var(--text)); }
  .switch:checked { background: var(--accent); }
  .switch:checked:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 88%, #000); }
  .switch:checked::after { transform: translateX(14px); }
  .switch:disabled { opacity: 0.45; }
  .switch:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
