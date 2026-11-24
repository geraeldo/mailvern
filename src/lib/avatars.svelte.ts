import { api, type SenderIcon } from "./api";
import { ui } from "./state.svelte";

const MAX_CACHE = 250;
/** Pictures by sender domain: missing while unknown, null when the domain has none. */
const icons = $state<Record<string, SenderIcon | null>>({});
/** Insertion order for bounded eviction. */
const order: string[] = [];
/** Domains with an in-flight or resolved lookup. */
const asked = new Set<string>();

function remember(domain: string, icon: SenderIcon | null) {
  if (!(domain in icons)) {
    order.push(domain);
    while (order.length > MAX_CACHE) {
      const oldest = order.shift()!;
      delete icons[oldest];
      asked.delete(oldest);
    }
  }
  icons[domain] = icon;
}

const domainOf = (email: string) => email.split("@").pop()!.trim().toLowerCase();

/**
 * The picture for a sender, or null to show initials. The first call for a domain asks
 * Mailvern (which looks it up once and caches it); the avatar updates when it arrives.
 */
export function senderIcon(email: string): SenderIcon | null {
  if (!ui.senderIcons || !email || !email.includes("@")) return null;
  const domain = domainOf(email);
  const known = icons[domain];
  if (known !== undefined) return known;
  if (!asked.has(domain)) {
    asked.add(domain);
    api.senderIcon(email).then(
      (icon) => remember(domain, icon),
      () => {
        // Don't permanently cache transient fetch failures.
        asked.delete(domain);
      },
    );
  }
  return null;
}

/** The address in a recipient like `Name <a@b.com>`. */
export function addrOf(recipient: string): string {
  return recipient.match(/<([^>]+)>/)?.[1] ?? recipient.trim();
}
