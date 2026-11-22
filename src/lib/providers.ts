export type Servers = { imapHost: string; imapPort: number; smtpHost: string; smtpPort: number };

const presets: Record<string, Servers> = {
  gmail: { imapHost: "imap.gmail.com", imapPort: 993, smtpHost: "smtp.gmail.com", smtpPort: 465 },
  icloud: { imapHost: "imap.mail.me.com", imapPort: 993, smtpHost: "smtp.mail.me.com", smtpPort: 587 },
  outlook: { imapHost: "outlook.office365.com", imapPort: 993, smtpHost: "smtp.office365.com", smtpPort: 587 },
  yahoo: { imapHost: "imap.mail.yahoo.com", imapPort: 993, smtpHost: "smtp.mail.yahoo.com", smtpPort: 465 },
  fastmail: { imapHost: "imap.fastmail.com", imapPort: 993, smtpHost: "smtp.fastmail.com", smtpPort: 465 },
  zoho: { imapHost: "imap.zoho.com", imapPort: 993, smtpHost: "smtp.zoho.com", smtpPort: 465 },
  proton: { imapHost: "127.0.0.1", imapPort: 1143, smtpHost: "127.0.0.1", smtpPort: 1025 },
};

const domains: Record<string, keyof typeof presets> = {
  "gmail.com": "gmail", "googlemail.com": "gmail",
  "icloud.com": "icloud", "me.com": "icloud", "mac.com": "icloud",
  "outlook.com": "outlook", "hotmail.com": "outlook", "live.com": "outlook", "msn.com": "outlook",
  "yahoo.com": "yahoo", "ymail.com": "yahoo",
  "fastmail.com": "fastmail", "fastmail.fm": "fastmail",
  "zoho.com": "zoho", "zohomail.com": "zoho",
  "proton.me": "proton", "protonmail.com": "proton",
};

/** Known provider settings, or a conventional imap./smtp. guess for custom domains. */
export function guessServers(email: string): Servers | null {
  const domain = email.split("@")[1]?.trim().toLowerCase();
  if (!domain || !domain.includes(".")) return null;
  const known = domains[domain];
  if (known) return { ...presets[known] };
  return { imapHost: `imap.${domain}`, imapPort: 993, smtpHost: `smtp.${domain}`, smtpPort: 465 };
}
