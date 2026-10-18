# Feedback from the website

The site's **Feedback** button posts the form to a Google Apps Script web app that adds one row
per message to a Google Sheet you own. Nobody else can read the sheet. Until the site has the
web app's URL (`FEEDBACK_URL` in `site/index.html`), the button opens a pre-filled GitHub issue
instead.

## Set it up (about 5 minutes)

1. Create a Google Sheet, for example "anchorleg feedback".
2. In the sheet, open **Extensions → Apps Script**, replace the editor's contents with the script
   below, and save.
3. **Deploy → New deployment** → type **Web app**. Set *Execute as*: **Me**, and *Who has
   access*: **Anyone**. Deploy, allow access when Google asks, and copy the **Web app URL**
   (`https://script.google.com/macros/s/…/exec`).
4. In `site/index.html`, set `var FEEDBACK_URL = '<that URL>';`, commit and push to `main`. The
   Pages workflow republishes the site.
5. Send yourself a test message from the site. A **Feedback** tab appears in the sheet with the
   row.

"Anyone" only means anyone can **send** to the script; it can only append rows. The script runs
as you, so it can write to your sheet, but visitors never see the sheet or its URL.

To change the script later, edit it, then **Deploy → Manage deployments → Edit → Version: New
version**. That keeps the same URL.

## The script

```js
// anchorleg website feedback → this sheet. One row per message.
const SHEET = 'Feedback';
const PER_HOUR = 60; // more than this in an hour is dropped (spam guard)

function doPost(e) {
  let d;
  try {
    d = JSON.parse((e && e.postData && e.postData.contents) || '{}');
  } catch (err) {
    return text('bad request');
  }
  if (d.website) return text('ok'); // the hidden field only bots fill in
  const message = String(d.message || '').trim().slice(0, 4000);
  if (!message) return text('empty');

  const cache = CacheService.getScriptCache();
  const seen = Number(cache.get('count') || 0);
  if (seen >= PER_HOUR) return text('busy');
  cache.put('count', String(seen + 1), 3600);

  const lock = LockService.getScriptLock();
  lock.waitLock(10000);
  try {
    const book = SpreadsheetApp.getActiveSpreadsheet();
    const sheet = book.getSheetByName(SHEET) || book.insertSheet(SHEET);
    if (sheet.getLastRow() === 0) {
      sheet.appendRow(['Received', 'Kind', 'Message', 'Email', 'Page', 'Came from', 'Browser', 'Language']);
      sheet.setFrozenRows(1);
    }
    sheet.appendRow([
      new Date(),
      cell(d.kind, 20),
      cell(message, 4000),
      cell(d.email, 200),
      cell(d.page, 200),
      cell(d.ref, 300),
      cell(d.ua, 300),
      cell(d.lang, 20),
    ]);
  } finally {
    lock.releaseLock();
  }
  // Optional: an email for every message. Uncomment and run once from the editor to allow it.
  // MailApp.sendEmail(Session.getEffectiveUser().getEmail(), 'anchorleg feedback: ' + cell(d.kind, 20), message);
  return text('ok');
}

// Text as the sheet should store it: short, and never read as a formula.
function cell(v, max) {
  const s = String(v == null ? '' : v).slice(0, max);
  return /^[=+\-@\t\r]/.test(s) ? "'" + s : s;
}

function text(s) {
  return ContentService.createTextOutput(s);
}
```

## What's collected

What the person types (kind, message, optional email), plus the page they sent it from, the
page that linked them there, their browser's user-agent and language. No cookies, no IP address
(Apps Script doesn't see it).

## Analytics

Visits and clicks are counted with [GoatCounter](https://anchorleg.goatcounter.com), without cookies
or personal data. Besides page views it records these events:

| Event | When |
|---|---|
| `copy-install` | the install command is copied |
| `out-desktop-download` | the desktop download button is clicked |
| `out-github-*`, `out-install-other`, `out-cli-docs`, `out-design` | links to GitHub and the docs |
| `feedback-open`, `feedback-sent`, `feedback-issue` | the feedback form is opened, sent, or sent as an issue |

Downloads themselves are counted by GitHub. Per file, for every release:

```bash
gh api repos/snehal96/anchorleg/releases --paginate \
  --jq '.[] | .tag_name as $t | .assets[] | "\($t)\t\(.download_count)\t\(.name)"'
```

Repository visitors, clones and where they came from: **Insights → Traffic** on GitHub (last 14
days; `gh api repos/snehal96/anchorleg/traffic/views` and `…/traffic/popular/referrers` keep the
same numbers).
