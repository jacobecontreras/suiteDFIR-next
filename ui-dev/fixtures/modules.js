// @ts-check
// Large, deterministic module lists for the mock (ModuleInfo shape from the contract fixtures), so
// the module picker can be exercised at real-world size (DEVELOPMENT.md §4.6: responsive at 1,300
// modules). The contract fixture's own modules (callHistory, sms) are always included.
import { ToolModules } from "./contracts/index.js";

/** @typedef {import("../../ui/types").ModuleInfo} ModuleInfo */
/** @typedef {import("../../ui/types").ToolId} ToolId */

const CATEGORIES = {
  ileapp: [
    "Accounts", "AirDrop", "Alarms", "App Conduit", "App Permissions", "Apple Mail", "Apple Maps",
    "Apple Music", "Apple Pay", "Apple Podcasts", "Biome", "Bluetooth", "Calendar", "Call History",
    "CarPlay", "Cellular", "Chrome", "Contacts", "Discord", "Downloads", "Facebook Messenger",
    "Find My", "Gmail", "Google Maps", "Health", "Home", "iCloud", "Installed Apps", "Instagram",
    "Keyboard", "KnowledgeC", "Locations", "Media Library", "Mobile Activation", "Notes",
    "Notifications", "Photos", "PowerLog", "Reminders", "Safari", "Screen Time", "Signal", "Slack",
    "SMS & iMessage", "Snapchat", "Spotlight", "Telegram", "TikTok", "Uber", "Venmo", "Voicemail",
    "Wallet", "WhatsApp", "Wi-Fi", "Zoom",
  ],
  aleapp: [
    "Accounts", "Android System", "App Usage", "Battery", "Bluetooth", "Call Logs", "Chrome",
    "Contacts", "Digital Wellbeing", "Discord", "Downloads", "Facebook Messenger", "Files by Google",
    "Gmail", "Google Maps", "Google Photos", "Installed Apps", "Instagram", "Keyboard", "Locations",
    "Media Store", "Notifications", "Package Manager", "Recent Activity", "Samsung", "Settings",
    "Signal", "Slack", "SMS & MMS", "Snapchat", "Telegram", "TikTok", "Usage Stats", "Waze",
    "WhatsApp", "Wi-Fi", "Wellbeing", "Zoom",
  ],
};

const SUBJECTS = [
  "Accounts", "Attachments", "Cache", "Call Records", "Contacts", "Cookies", "Databases",
  "Device Info", "Downloads", "Events", "Favorites", "Files", "History", "Locations", "Logs",
  "Media", "Messages", "Notifications", "Preferences", "Searches", "Sessions", "Settings",
  "Thumbnails", "Transactions", "Usage",
];

/**
 * @param {string} s
 * @returns {string} lowerCamelCase of the words in `s`
 */
function camel(s) {
  const words = s.replace(/&/g, "and").split(/[^A-Za-z0-9]+/).filter(Boolean);
  return words
    .map((w, i) => (i === 0 ? w.charAt(0).toLowerCase() + w.slice(1) : w.charAt(0).toUpperCase() + w.slice(1)))
    .join("");
}

/**
 * `count` modules for `tool`, sorted by category, then display name (case-insensitive), like
 * `modules.json` (CONTRACTS.md §5). Category sizes vary (about 1:11), as in the real lists.
 * @param {ToolId} tool
 * @param {number} count
 * @returns {ModuleInfo[]}
 */
export function makeModules(tool, count) {
  /** @type {ModuleInfo[]} */
  const out = tool === "ileapp" ? structuredClone(ToolModules.modules) : [];
  const names = new Set(out.map((m) => m.name));
  const categories = CATEGORIES[tool];
  const weights = categories.map((_, c) => 1 + ((c * 7) % 11));
  const total = weights.reduce((a, b) => a + b, 0);
  const wanted = count - out.length;
  const sizes = weights.map((w) => Math.floor((wanted * w) / total));
  for (let c = 0; sizes.reduce((a, b) => a + b, 0) < wanted; c = (c + 1) % sizes.length) sizes[c] += 1;
  categories.forEach((category, c) => {
    const moduleName = camel(category);
    for (let k = 0, made = 0; made < sizes[c]; k++) {
      const subject = SUBJECTS[(k + c) % SUBJECTS.length];
      const variant = Math.floor(k / SUBJECTS.length);
      const suffix = variant === 0 ? "" : ` ${variant + 1}`;
      const name = `${moduleName}${camel(subject).replace(/^./, (x) => x.toUpperCase())}${variant === 0 ? "" : variant + 1}`;
      if (names.has(name)) continue;
      names.add(name);
      made += 1;
      out.push({
        name,
        module_name: moduleName,
        category,
        display_name: `${category} ${subject}${suffix}`,
        description: (c + k) % 3 === 0 ? null : `${subject} parsed from ${category} data`,
      });
    }
  });
  const key = (/** @type {string} */ s) => s.toLowerCase();
  out.sort(
    (a, b) =>
      key(a.category).localeCompare(key(b.category)) || key(a.display_name).localeCompare(key(b.display_name)),
  );
  return out;
}
