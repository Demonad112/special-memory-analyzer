use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::explorer;

/// Longest list drawn in the HTML page; the CSV always has every row.
pub const MAX_HTML_ROWS: usize = 5_000;
/// Rows a printed table keeps before "see the CSV".
const MAX_PRINT_ROWS: usize = 200;

/// A report laid out for export: header, summary tiles, notes and tables.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    /// "Compare", "Record a copy", ...
    pub title: String,
    /// What it was about: "C:\Data → \\nas\backup".
    pub subtitle: String,
    /// Label and value pairs under the title: client, ticket, date, machine, ...
    pub meta: Vec<(String, String)>,
    /// The pill in the header; set by the builder of each report kind.
    pub verdict: Option<Verdict>,
    pub tiles: Vec<Tile>,
    pub notes: Vec<String>,
    /// A headline bar under the tiles (Compare: how much of the source is at the destination).
    pub share: Option<Share>,
    /// The first table is the one exported as CSV.
    pub tables: Vec<Table>,
    /// Exported as the CSV instead of the first table (Record: every file with its status).
    pub csv_table: Option<Table>,
    /// Drawn after the tables: collapsible blocks, code blocks.
    pub sections: Vec<Section>,
    /// Machine-readable data for the page script, embedded as a JSON island.
    pub data: Option<serde_json::Value>,
    /// Draw the disk usage explorer (folder drill-down and type filter) from `data`. It has its
    /// own script and styles, so the shared ones stay as they are.
    pub explorer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Good,
    Bad,
}

/// A block after the tables.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    /// A native `<details>`: works with scripts off, and the keyboard.
    Details {
        summary: String,
        open: bool,
        body: Vec<Section>,
    },
    Table(Table),
    Code(CodeBlock),
    Note(String),
    /// The Compare "copy the missing files again" block; see `recovery.rs`.
    Recovery(Recovery),
}

/// The text of a recovery block. The page script rebuilds the command from the `rec` entry of
/// [`Document::data`] when an Exclude preset is ticked; this is what shows with scripts off.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovery {
    pub heading: String,
    pub intro: String,
    /// The command, cut to the first folders when the list is long.
    pub script: String,
    /// The second command: copies into a new folder on this PC.
    pub script_pass2: String,
    /// `script` stops before the last folders; the download has them all.
    pub truncated: bool,
    /// Preset bit and label, for the presets that match at least one file.
    pub presets: Vec<(usize, String)>,
    /// Paths a script cannot carry, to copy by hand.
    pub manual: Vec<String>,
}

/// Text to copy or save (a script): a read-only box with Copy and Save buttons.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBlock {
    pub label: String,
    pub text: String,
    /// File name for the Save button.
    pub download_name: String,
}

/// A labelled progress bar: `percent` is 0 to 100.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub label: String,
    pub detail: String,
    pub percent: f64,
    pub tone: Tone,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    pub label: String,
    pub value: String,
    pub detail: String,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    #[default]
    Plain,
    Good,
    Warn,
    Bad,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub heading: String,
    pub columns: Vec<String>,
    /// Columns shown right-aligned (sizes, counts).
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<String>>,
    /// Per-row colour; rows past the end are plain.
    pub row_tone: Option<Vec<Tone>>,
    /// The column holding a formatted size; the CSV adds a raw "Bytes" column after it.
    pub size_col: Option<usize>,
    /// Raw byte counts, one per row, for `size_col`.
    pub row_bytes: Vec<u64>,
    /// Said under the table when only part of the list is in the report.
    pub footnote: String,
}

impl Table {
    /// Draws every row in `tone`.
    #[must_use]
    pub fn toned(mut self, tone: Tone) -> Self {
        self.row_tone = Some(vec![tone; self.rows.len()]);
        self
    }

    fn tone_of(&self, row: usize) -> Tone {
        self.row_tone
            .as_ref()
            .and_then(|tones| tones.get(row))
            .copied()
            .unwrap_or_default()
    }
}

impl Document {
    /// A standalone HTML page (inline styles and one script, prints well) headed with `brand`.
    ///
    /// The page carries a Content-Security-Policy that allows only its own script (by hash), so
    /// it cannot reach the network whatever the data holds.
    pub fn html(&self, brand: &str, generated: &str) -> String {
        let brand = if brand.trim().is_empty() {
            "Deep Server Analyzer"
        } else {
            brand.trim()
        };
        let mut out = String::with_capacity(16 * 1024);
        out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        out.push_str(&format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">\n",
            csp_for(self.explorer)
        ));
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        out.push_str(&format!(
            "<title>{} - {}</title>\n<style>{STYLE}</style>\n{}</head>\n<body>\n<main>\n",
            esc(&self.title),
            esc(brand),
            if self.explorer {
                format!("<style>{}</style>\n", explorer::STYLE)
            } else {
                String::new()
            }
        ));
        let verdict = match self.verdict {
            Some(Verdict::Bad) => Some(("bad", "Needs attention")),
            Some(Verdict::Good) => Some(("good", "All clear")),
            None => None,
        };
        out.push_str(&format!(
            "<header><div class=\"top\"><p class=\"brand\">{}</p>{}</div><h1>{}</h1><p class=\"subtitle\">{}</p>",
            esc(brand),
            verdict.map_or(String::new(), |(tone, text)| format!(
                "<span class=\"pill {tone}\">{text}</span>"
            )),
            esc(&self.title),
            esc(&self.subtitle)
        ));
        if !self.meta.is_empty() {
            out.push_str("<dl>");
            for (label, value) in &self.meta {
                out.push_str(&format!(
                    "<div><dt>{}</dt><dd>{}</dd></div>",
                    esc(label),
                    esc(value)
                ));
            }
            out.push_str("</dl>");
        }
        out.push_str("</header>\n");

        if !self.tiles.is_empty() {
            out.push_str("<section class=\"tiles\">");
            for tile in &self.tiles {
                out.push_str(&format!(
                    "<div class=\"tile {}\"><p class=\"label\">{}</p><p class=\"value\">{}</p><p class=\"detail\">{}</p></div>",
                    tone_class(tile.tone),
                    esc(&tile.label),
                    esc(&tile.value),
                    esc(&tile.detail)
                ));
            }
            out.push_str("</section>\n");
        }
        if let Some(share) = &self.share {
            out.push_str(&format!(
                "<section class=\"share {}\"><div class=\"head\"><strong>{}</strong><span>{}</span></div><div class=\"bar\"><i style=\"width:{:.1}%\"></i></div></section>\n",
                tone_class(share.tone),
                esc(&share.label),
                esc(&share.detail),
                share.percent.clamp(0.0, 100.0)
            ));
        }
        for note in &self.notes {
            out.push_str(&format!("<p class=\"note\">{}</p>\n", esc(note)));
        }
        if self.explorer {
            // Built by the explorer script; without scripts the flat tables below say it all.
            out.push_str("<section class=\"card explorer\" id=\"x\" data-js hidden></section>\n");
        }
        let mut ids = 0;
        for table in &self.tables {
            render_table(&mut out, table, &mut ids);
        }
        for section in &self.sections {
            render_section(&mut out, section, &mut ids);
        }
        if let Some(data) = &self.data {
            out.push_str(&format!(
                "<script type=\"application/json\" id=\"d\">{}</script>\n",
                json_island(data)
            ));
        }
        out.push_str(&format!(
            "<footer><span>Made with Deep Server Analyzer</span><span>{}</span></footer>\n</main>\n<script>{SCRIPT}</script>\n{}</body>\n</html>\n",
            esc(generated),
            if self.explorer {
                format!("<script>{}</script>\n", explorer::SCRIPT)
            } else {
                String::new()
            }
        ));
        out
    }

    /// `csv_table`, or else the first table, as CSV for Excel: UTF-8 with a BOM, CRLF line ends. A table with a size
    /// column gets a raw "Bytes" column after it.
    pub fn csv(&self) -> String {
        let mut out = String::from("\u{feff}");
        let Some(table) = self.csv_table.as_ref().or(self.tables.first()) else {
            return out;
        };
        let mut line = |cells: &[String]| {
            let row: Vec<String> = cells.iter().map(|cell| csv_cell(cell)).collect();
            out.push_str(&row.join(","));
            out.push_str("\r\n");
        };
        let with_bytes = |cells: &[String], bytes: Option<String>| -> Vec<String> {
            let mut cells = cells.to_vec();
            if let (Some(col), Some(bytes)) = (table.size_col, bytes) {
                cells.insert((col + 1).min(cells.len()), bytes);
            }
            cells
        };
        line(&with_bytes(&table.columns, Some("Bytes".into())));
        for (index, row) in table.rows.iter().enumerate() {
            let bytes = table.row_bytes.get(index).map(u64::to_string);
            line(&with_bytes(row, bytes));
        }
        out
    }
}

fn render_table(out: &mut String, table: &Table, ids: &mut usize) {
    *ids += 1;
    let id = format!("t{ids}");
    out.push_str(&format!(
        "<section class=\"card\"><h2>{}<span class=\"count\">{}</span></h2>",
        esc(&table.heading),
        table.rows.len()
    ));
    if table.rows.is_empty() {
        out.push_str("<p class=\"empty\">Nothing to list.</p></section>\n");
        return;
    }
    if table.rows.len() > 10 {
        out.push_str(&format!(
            "<p class=\"tools\" data-js hidden><input type=\"search\" data-filter=\"{id}\" placeholder=\"Filter this list\" aria-label=\"Filter {}\"><span id=\"{id}n\" aria-live=\"polite\"></span></p>",
            esc(&table.heading)
        ));
    }
    let shown = table.rows.len().min(MAX_HTML_ROWS);
    let long = if table.rows.len() > MAX_PRINT_ROWS {
        " class=\"long\""
    } else {
        ""
    };
    out.push_str(&format!(
        "<div class=\"scroll\"><table id=\"{id}\"{long}><thead><tr>"
    ));
    for (index, column) in table.columns.iter().enumerate() {
        out.push_str(&format!(
            "<th{}>{}</th>",
            num_class(table, index),
            esc(column)
        ));
    }
    out.push_str("</tr></thead><tbody>");
    for (row_index, row) in table.rows.iter().take(shown).enumerate() {
        match table.tone_of(row_index) {
            Tone::Plain => out.push_str("<tr>"),
            tone => out.push_str(&format!("<tr class=\"{}\">", tone_class(tone))),
        }
        for (index, cell) in row.iter().enumerate() {
            out.push_str(&format!(
                "<td{}>{}</td>",
                num_class(table, index),
                esc(cell)
            ));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    if shown < table.rows.len() {
        out.push_str(&format!(
            "<p class=\"note\">{} more not shown here. Export the CSV for the full list.</p>",
            table.rows.len() - shown
        ));
    }
    if table.rows.len() > MAX_PRINT_ROWS {
        out.push_str(&format!(
            "<p class=\"note printonly\">Only the first {MAX_PRINT_ROWS} rows are printed. See the CSV for all {}.</p>",
            table.rows.len()
        ));
    }
    if !table.footnote.is_empty() {
        out.push_str(&format!("<p class=\"note\">{}</p>", esc(&table.footnote)));
    }
    out.push_str("</section>\n");
}

fn render_section(out: &mut String, section: &Section, ids: &mut usize) {
    match section {
        Section::Details {
            summary,
            open,
            body,
        } => {
            out.push_str(&format!(
                "<details class=\"card fold\"{}><summary>{}</summary>",
                if *open { " open" } else { "" },
                esc(summary)
            ));
            for part in body {
                render_section(out, part, ids);
            }
            out.push_str("</details>\n");
        }
        Section::Table(table) => render_table(out, table, ids),
        Section::Code(block) => {
            *ids += 1;
            let id = format!("c{ids}");
            out.push_str(&format!(
                "<div class=\"code\"><p class=\"label\">{}</p><textarea id=\"{id}\" readonly rows=\"8\" spellcheck=\"false\" aria-label=\"{}\">{}</textarea>",
                esc(&block.label),
                esc(&block.label),
                esc(&block.text)
            ));
            out.push_str(&format!(
                "<p class=\"tools\" data-js hidden><button type=\"button\" data-copy=\"{id}\">Copy</button>"
            ));
            if !block.download_name.is_empty() {
                out.push_str(&format!(
                    "<button type=\"button\" data-save=\"{id}\" data-name=\"{}\">Save as file</button>",
                    esc(&block.download_name)
                ));
            }
            out.push_str("</p></div>\n");
        }
        Section::Note(text) => out.push_str(&format!("<p class=\"note\">{}</p>\n", esc(text))),
        Section::Recovery(rec) => render_recovery(out, rec),
    }
}

fn render_recovery(out: &mut String, rec: &Recovery) {
    out.push_str(&format!(
        "<section class=\"card rec\" id=\"rec\"><h2>{}</h2><p class=\"note\">{}</p>",
        esc(&rec.heading),
        esc(&rec.intro)
    ));
    if !rec.script.is_empty() {
        if !rec.presets.is_empty() {
            out.push_str("<fieldset class=\"presets act\" data-js hidden><legend>Leave these out (all off to start)</legend>");
            for (bit, label) in &rec.presets {
                out.push_str(&format!(
                    "<label><input type=\"checkbox\" data-bit=\"{bit}\"> {}</label>",
                    esc(label)
                ));
            }
            out.push_str("</fieldset>");
        }
        out.push_str(
            "<p class=\"tools\" data-js hidden><button type=\"button\" data-copy=\"rec-main\">Copy</button></p>",
        );
        let cut = if rec.truncated { " data-cut=\"1\"" } else { "" };
        out.push_str(&format!(
            "<details class=\"cmd act\"><summary>Show the full command</summary><textarea id=\"rec-main\" rows=\"12\" spellcheck=\"false\" aria-label=\"PowerShell command\"{cut}>{}</textarea>",
            esc(&rec.script)
        ));
        if rec.truncated {
            out.push_str("<p class=\"note\">The box shows the first folders only. Download .ps1 has all of them.</p>");
        }
        out.push_str(
            "<p class=\"tools\" data-js hidden><button type=\"button\" data-copy=\"rec-main\">Copy</button><button type=\"button\" data-ps1=\"rec-main\" data-name=\"Copy-missing-files.ps1\">Download .ps1</button></p></details>",
        );
        out.push_str(
            "<label class=\"tools act\" data-js hidden><input type=\"checkbox\" id=\"rec-p2\"> Copy pass #2 still didn't work</label>",
        );
        out.push_str(&format!(
            "<div id=\"rec-p2box\" class=\"act\" hidden><p class=\"note\">This copies the same files into a new folder on this PC (Downloads\\Missed File Transfers, or the temp folder if that fails) and opens it. Hand that folder to the client.</p><textarea id=\"rec-pass2\" rows=\"12\" spellcheck=\"false\" aria-label=\"PowerShell command, pass 2\"{cut}>{}</textarea><p class=\"tools\"><button type=\"button\" data-copy=\"rec-pass2\">Copy</button><button type=\"button\" data-ps1=\"rec-pass2\" data-name=\"Copy-missing-files-to-this-PC.ps1\">Download .ps1</button></p></div>",
            esc(&rec.script_pass2)
        ));
    }
    if !rec.manual.is_empty() {
        out.push_str(&format!(
            "<details class=\"cmd\"><summary>{} to copy by hand</summary><p class=\"note\">These names have characters a script cannot carry safely. Copy them in Explorer.</p><ul class=\"manual\">",
            rec.manual.len()
        ));
        for path in rec.manual.iter().take(MAX_HTML_ROWS) {
            out.push_str(&format!("<li>{}</li>", esc(path)));
        }
        out.push_str("</ul>");
        if rec.manual.len() > MAX_HTML_ROWS {
            out.push_str(&format!(
                "<p class=\"note\">{} more not shown here.</p>",
                rec.manual.len() - MAX_HTML_ROWS
            ));
        }
        out.push_str("</details>");
    }
    out.push_str("</section>\n");
}

/// The policy for a plain page: styles inline, images from data, and no script but ours.
#[cfg(test)]
fn csp() -> String {
    csp_for(false)
}

/// The policy for a page; an explorer page also allows its own script, by hash.
fn csp_for(explorer: bool) -> String {
    let hash = |script: &str| STANDARD.encode(Sha256::digest(script.as_bytes()));
    let mut scripts = format!("'sha256-{}'", hash(SCRIPT));
    if explorer {
        scripts.push_str(&format!(" 'sha256-{}'", hash(explorer::SCRIPT)));
    }
    format!("default-src 'none'; style-src 'unsafe-inline'; script-src {scripts}; img-src data:")
}

/// JSON safe to put inside a `<script>` element.
fn json_island(data: &serde_json::Value) -> String {
    let json = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(ch),
        }
    }
    out
}

fn csv_cell(cell: &str) -> String {
    // A leading = + - @ (or tab / CR) would run as a formula in Excel.
    let cell = if cell.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{cell}")
    } else {
        cell.to_string()
    };
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell
    }
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Plain => "",
        Tone::Good => "good",
        Tone::Warn => "warn",
        Tone::Bad => "bad",
    }
}

fn num_class(table: &Table, index: usize) -> &'static str {
    if table.numeric.get(index).copied().unwrap_or(false) {
        " class=\"num\""
    } else {
        ""
    }
}

/// The page script: list filter, Copy, Save. Everything else is native HTML.
const SCRIPT: &str = "
(function(){
var q=function(s,r){return Array.prototype.slice.call((r||document).querySelectorAll(s))};
q('[data-js]').forEach(function(e){e.hidden=false});
q('input[data-filter]').forEach(function(i){
var t=document.getElementById(i.getAttribute('data-filter')),c=document.getElementById(t.id+'n');
i.addEventListener('input',function(){
var v=i.value.toLowerCase(),n=0;
q('tbody tr',t).forEach(function(r){var h=v!==''&&r.textContent.toLowerCase().indexOf(v)<0;r.hidden=h;if(!h)n++});
c.textContent=v===''?'':n+' shown'})});
q('button[data-copy]').forEach(function(b){b.addEventListener('click',function(){
var t=document.getElementById(b.getAttribute('data-copy'));t.select();
var done=function(){b.textContent='Copied'};
if(navigator.clipboard){navigator.clipboard.writeText(t.value).then(done,function(){document.execCommand('copy');done()})}
else{document.execCommand('copy');done()}})});
q('button[data-save]').forEach(function(b){b.addEventListener('click',function(){
var t=document.getElementById(b.getAttribute('data-save')),a=document.createElement('a');
a.href=URL.createObjectURL(new Blob([t.value],{type:'text/plain'}));a.download=b.getAttribute('data-name');
document.body.appendChild(a);a.click();a.remove()})});
var D=document.getElementById('d'),R=null;
try{R=D&&JSON.parse(D.textContent).rec}catch(e){}
if(R){
var cut=false,ed={},m=document.getElementById('rec-main'),p=document.getElementById('rec-pass2');
var mask=function(){var k=0;q('input[data-bit]').forEach(function(c){if(c.checked)k|=1<<c.getAttribute('data-bit')});return k};
var build=function(p2,k,lim){
var L=[p2?R.pre2:R.pre],n=0;cut=false;
R.dirs.forEach(function(d){
var names=d.f.filter(function(f){return !(f[1]&k)}).map(function(f){return f[0]});
if(!names.length)return;
if(lim&&n>=lim){cut=true;return}
n++;
var h=p2?d.h2:d.h,line=h;
names.forEach(function(s){
if(line.length+s.length>R.max&&line!==h){L.push(line+R.sfx,R.chk);line=h}
line+=' '+s});
L.push(line+R.sfx,R.chk)});
R.mk.forEach(function(e){if(!(e[2]&k))L.push(p2?e[1]:e[0])});
L.push(p2?R.post2:R.post);
return L.join('\\n')+'\\n'};
var mark=function(t){if(cut)t.setAttribute('data-cut','1');else t.removeAttribute('data-cut')};
[m,p].forEach(function(t){if(t)t.addEventListener('input',function(){ed[t.id]=true})});
q('input[data-bit]').forEach(function(c){c.addEventListener('change',function(){
var k=mask();m.value=build(0,k,R.limit);mark(m);p.value=build(1,k,R.limit);mark(p);ed={}})});
var x=document.getElementById('rec-p2');
if(x)x.addEventListener('change',function(){document.getElementById('rec-p2box').hidden=!x.checked});
q('button[data-ps1]').forEach(function(b){b.addEventListener('click',function(){
var id=b.getAttribute('data-ps1'),t=document.getElementById(id),v=t.value;
if(t.getAttribute('data-cut')&&!ed[id])v=build(id==='rec-pass2'?1:0,mask(),0);
var a=document.createElement('a');
a.href=URL.createObjectURL(new Blob(['\\ufeff'+v.replace(/\\r?\\n/g,'\\r\\n')],{type:'text/plain'}));a.download=b.getAttribute('data-name');
document.body.appendChild(a);a.click();a.remove()})})}
})();
";

const STYLE: &str = "
:root{color-scheme:light dark;--page:#f3f5f8;--surface:#fff;--text:#14181f;--muted:#5b6472;--line:#e3e7ee;--zebra:#f8fafc;--accent:#0b5cad;--accent-2:#2f86e0;--good:#0f7b3f;--good-bg:#e6f6ec;--warn:#9a5b00;--warn-bg:#fff4dc;--bad:#c42b1c;--bad-bg:#fdecea;--bad-line:#f3b9b2}
@media (prefers-color-scheme:dark){:root{--page:#0f1319;--surface:#181e27;--text:#e8ecf2;--muted:#9aa5b5;--line:#2a3340;--zebra:#1c232d;--accent:#7ab6f5;--accent-2:#4a98ea;--good:#58c98a;--good-bg:#12301f;--warn:#f0b64a;--warn-bg:#33270f;--bad:#ff8a7d;--bad-bg:#35181a;--bad-line:#6b2c2c}}
*{box-sizing:border-box}
[hidden]{display:none!important}
body{margin:0;background:var(--page);color:var(--text);font:14px/1.5 'Segoe UI Variable Text','Segoe UI',system-ui,sans-serif;overflow-wrap:anywhere}
main{max-width:1000px;margin:0 auto;padding:24px 16px 40px}
header{background:linear-gradient(135deg,#0b3d75,#1a73c8);color:#fff;border-radius:14px;padding:24px 28px;margin-bottom:16px;box-shadow:0 6px 24px rgba(11,61,117,.25)}
.top{display:flex;justify-content:space-between;align-items:center;gap:12px}
.brand{margin:0;font-weight:600;letter-spacing:.08em;text-transform:uppercase;font-size:12px;opacity:.85}
.pill{border-radius:999px;padding:3px 12px;font-size:12px;font-weight:600;background:rgba(255,255,255,.18);border:1px solid rgba(255,255,255,.45)}
.pill.good{background:#d7f5e3;color:#0b5a2d;border-color:#d7f5e3}.pill.bad{background:#ffe0dc;color:#9c1f12;border-color:#ffe0dc}
h1{margin:10px 0 0;font:600 30px/1.15 'Segoe UI Variable Display','Segoe UI',system-ui,sans-serif}
.subtitle{margin:6px 0 0;opacity:.9;font-size:15px}
dl{display:flex;flex-wrap:wrap;gap:6px 28px;margin:16px 0 0;padding-top:14px;border-top:1px solid rgba(255,255,255,.25)}
dl div{display:flex;flex-direction:column}dt{font-size:11px;text-transform:uppercase;letter-spacing:.06em;opacity:.75}dd{margin:0;font-weight:600}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:12px;margin-bottom:16px}
.tile{background:var(--surface);border:1px solid var(--line);border-top:4px solid var(--accent-2);border-radius:10px;padding:12px 16px;box-shadow:0 1px 3px rgba(0,0,0,.05)}
.tile p{margin:0}.label{color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:.05em}.detail{color:var(--muted);font-size:12px}
.value{font-size:24px;font-weight:600;font-variant-numeric:tabular-nums;margin:2px 0}
.tile.good{border-top-color:var(--good)}.tile.good .value{color:var(--good)}
.tile.warn{border-top-color:var(--warn)}.tile.warn .value{color:var(--warn)}
.tile.bad{border-top-color:var(--bad);background:var(--bad-bg);border-color:var(--bad-line)}.tile.bad .value{color:var(--bad)}
.share{background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:12px 16px;margin-bottom:16px}
.share .head{display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-bottom:8px}.share .head span{color:var(--muted)}
.bar{height:12px;border-radius:999px;background:var(--line);overflow:hidden}.bar i{display:block;height:100%;border-radius:999px;background:linear-gradient(90deg,var(--accent),var(--accent-2))}
.share.good .bar i{background:var(--good)}.share.bad .bar i{background:var(--bad)}
.note{color:var(--muted);font-size:13px;margin:8px 4px}.empty{color:var(--muted);padding:8px 0}.printonly{display:none}
.card{background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:8px 16px 12px;margin-bottom:16px;box-shadow:0 1px 3px rgba(0,0,0,.05)}
h2{font-size:16px;margin:10px 0;display:flex;align-items:center;gap:10px}
.count{font-size:12px;font-weight:600;color:var(--accent);background:var(--page);border-radius:999px;padding:1px 10px}
details.fold>summary{cursor:pointer;font-size:16px;font-weight:600;padding:8px 0;border-radius:6px}
details.fold>summary:focus-visible,button:focus-visible,input:focus-visible,textarea:focus-visible{outline:2px solid var(--accent-2);outline-offset:2px}
.tools{display:flex;gap:8px;align-items:center;margin:6px 0;color:var(--muted);font-size:12px}
input[type=search]{flex:1;max-width:320px;padding:6px 10px;border:1px solid var(--line);border-radius:6px;background:var(--surface);color:var(--text);font:inherit}
button{padding:6px 14px;border:1px solid var(--line);border-radius:6px;background:var(--surface);color:var(--text);font:inherit;cursor:pointer}
button:hover{border-color:var(--accent-2)}
.code textarea{width:100%;font:12px/1.4 Consolas,'Cascadia Mono',monospace;background:var(--zebra);color:var(--text);border:1px solid var(--line);border-radius:6px;padding:8px;resize:vertical;white-space:pre;overflow-wrap:normal}
.rec fieldset{border:1px solid var(--line);border-radius:8px;margin:8px 0;padding:6px 12px;display:flex;flex-wrap:wrap;gap:6px 18px}.rec legend{color:var(--muted);font-size:12px;padding:0 4px}.rec label{display:flex;gap:6px;align-items:center}
.rec details.cmd{margin:8px 0}.rec details.cmd>summary{cursor:pointer;font-weight:600;padding:4px 0}.rec textarea{width:100%;font:12px/1.4 Consolas,'Cascadia Mono',monospace;background:var(--zebra);color:var(--text);border:1px solid var(--line);border-radius:6px;padding:8px;resize:vertical;white-space:pre;overflow-wrap:normal}
.rec ul.manual{margin:6px 0;padding-left:20px;font-family:Consolas,'Cascadia Mono',monospace;font-size:12px}
.scroll{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:13px}
th{position:sticky;top:0;background:var(--surface);text-align:left;color:var(--muted);font-size:11px;text-transform:uppercase;letter-spacing:.05em;border-bottom:2px solid var(--line);padding:8px}
td{border-bottom:1px solid var(--line);padding:6px 8px;vertical-align:top}
tbody tr:nth-child(even){background:var(--zebra)}
.num{text-align:right;white-space:nowrap;font-variant-numeric:tabular-nums}
tr.bad td{color:var(--bad)}tr.bad td:first-child{border-left:3px solid var(--bad)}
tr.warn td{color:var(--warn)}tr.warn td:first-child{border-left:3px solid var(--warn)}
tr.good td:first-child{border-left:3px solid var(--good)}
footer{display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-top:24px;color:var(--muted);font-size:12px}
@media (max-width:560px){header{padding:18px}h1{font-size:24px}.value{font-size:20px}}
@media print{:root{--page:#fff}body{background:#fff}main{padding:0}header{box-shadow:none;-webkit-print-color-adjust:exact;print-color-adjust:exact}.tile,.card,tr{break-inside:avoid;box-shadow:none}th{position:static}.tools,.code button,.rec .act{display:none}table.long tbody tr:nth-child(n+201){display:none}.printonly{display:block}details.fold:not([open])>*:not(summary){display:block}}
";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Document {
        Document {
            title: "Compare".into(),
            subtitle: r"C:\A & B → D:\<copy>".into(),
            meta: vec![("Client".into(), "O'Brien & Co".into())],
            verdict: Some(Verdict::Bad),
            tiles: vec![Tile {
                label: "Missing".into(),
                value: "2 files".into(),
                tone: Tone::Bad,
                ..Tile::default()
            }],
            tables: vec![Table {
                heading: "Missing at destination".into(),
                columns: vec!["Path".into(), "Size".into()],
                numeric: vec![false, true],
                rows: vec![
                    vec!["a, \"quoted\".txt".into(), "1 KB".into()],
                    vec!["=cmd|' /C calc'!A0".into(), "2 KB".into()],
                ],
                size_col: Some(1),
                row_bytes: vec![1024, 2048],
                ..Table::default()
            }
            .toned(Tone::Bad)],
            ..Document::default()
        }
    }

    /// Names that have broken report pages, shell scripts and spreadsheets before.
    const HOSTILE: &[&str] = &[
        "\"quote\".txt",
        "it's.txt",
        "<script>alert(1)</script>.txt",
        "</script><img src=x onerror=alert(1)>.txt",
        "back`tick`.txt",
        "$(calc).txt",
        "a&b.txt",
        "100%.txt",
        "bang!.txt",
        "caret^.txt",
        "line\nbreak.txt",
        "rtl\u{202e}txt.exe",
        "emoji 📁.txt",
        "=HYPERLINK(\"http://x\").txt",
        "\tTabbed.txt",
        "sep\u{2028}line.txt",
    ];

    fn hostile_doc() -> Document {
        let long = format!("{}.txt", "x".repeat(300));
        let mut names: Vec<String> = HOSTILE.iter().map(|name| (*name).to_string()).collect();
        names.push(long);
        Document {
            title: "Hostile".into(),
            verdict: Some(Verdict::Bad),
            tables: vec![Table {
                heading: "Names".into(),
                columns: vec!["Path".into()],
                numeric: vec![false],
                rows: names.iter().map(|name| vec![name.clone()]).collect(),
                ..Table::default()
            }],
            data: Some(serde_json::json!({ "names": names })),
            ..Document::default()
        }
    }

    #[test]
    fn html_is_branded_and_escaped() {
        let html = sample().html("Acme IT <script>", "Oct 3, 2026");
        assert!(html.contains("<p class=\"brand\">Acme IT &lt;script&gt;</p>"));
        assert!(html.contains(r"C:\A &amp; B → D:\&lt;copy&gt;"));
        assert!(html.contains("O&#39;Brien &amp; Co"));
        assert!(html.contains("<tr class=\"bad\">"));
        assert!(html.contains("<td class=\"num\">1 KB</td>"));
        assert!(sample()
            .html("  ", "x")
            .contains("<p class=\"brand\">Deep Server Analyzer</p>"));
    }

    #[test]
    fn html_shows_verdict_share_bar_and_row_count() {
        let mut doc = sample();
        doc.share = Some(Share {
            label: "Found".into(),
            detail: "8 of 10".into(),
            percent: 180.0,
            tone: Tone::Plain,
        });
        let html = doc.html("Acme", "x");
        assert!(html.contains("<span class=\"pill bad\">Needs attention</span>"));
        assert!(html.contains("<i style=\"width:100.0%\"></i>"));
        assert!(html.contains("<span class=\"count\">2</span>"));
        assert!(html.contains("prefers-color-scheme:dark"));
        doc.verdict = Some(Verdict::Good);
        assert!(doc.html("Acme", "x").contains("All clear"));
        doc.verdict = None;
        assert!(!doc.html("Acme", "x").contains("class=\"pill"));
    }

    #[test]
    fn csv_quotes_defuses_formulas_and_adds_raw_bytes() {
        let csv = sample().csv();
        assert!(csv.starts_with('\u{feff}'));
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(lines[0], "Path,Size,Bytes");
        assert_eq!(lines[1], "\"a, \"\"quoted\"\".txt\",1 KB,1024");
        assert_eq!(lines[2], "'=cmd|' /C calc'!A0,2 KB,2048");
        assert_eq!(Document::default().csv(), "\u{feff}");
        assert_eq!(csv_cell("\tx"), "'\tx");
        assert_eq!(csv_cell("\rx"), "\"'\rx\"");
    }

    #[test]
    fn csp_hash_matches_the_script_and_blocks_everything_else() {
        let html = sample().html("Acme", "x");
        let policy = csp();
        assert!(html.contains(&format!("content=\"{policy}\"")));
        assert!(policy.starts_with("default-src 'none'"));
        assert!(!policy.contains("unsafe-eval"));
        assert!(!policy.contains("script-src 'unsafe-inline'"));
        assert!(html.contains(&format!("<script>{SCRIPT}</script>")));
        // One executable script only; the JSON island is data.
        assert_eq!(html.matches("<script>").count(), 1);
    }

    #[test]
    fn explorer_pages_allow_exactly_two_scripts_by_hash() {
        let mut doc = sample();
        doc.explorer = true;
        doc.data = Some(serde_json::json!({ "x": { "nodes": [] } }));
        let html = doc.html("Acme", "x");
        let policy = csp_for(true);
        assert!(html.contains(&format!("content=\"{policy}\"")));
        assert_eq!(policy.matches("'sha256-").count(), 2);
        assert!(!policy.contains("unsafe-eval") && !policy.contains("script-src 'unsafe-inline'"));
        assert!(html.contains(&format!("<script>{SCRIPT}</script>")));
        assert!(html.contains(&format!("<script>{}</script>", explorer::SCRIPT)));
        assert!(html.contains("id=\"x\" data-js hidden"));
        // Two executable scripts and the JSON island; nothing else.
        assert_eq!(html.matches("<script>").count(), 2);
        assert_eq!(html.matches("<script").count(), 3);
        // The flat tables stay for readers without scripts.
        assert!(html.contains("Missing at destination"));
        // A plain page is unchanged: one script, one hash, no explorer styles.
        let plain = sample().html("Acme", "x");
        assert_eq!(csp().matches("'sha256-").count(), 1);
        assert_eq!(plain.matches("<script>").count(), 1);
        assert!(!plain.contains("class=\"xcols\"") && !plain.contains(".xcols"));
    }

    #[test]
    fn hostile_names_cannot_break_out_of_html_json_or_csv() {
        let doc = hostile_doc();
        let html = doc.html("Acme", "x");
        // No name introduced markup: the only script is ours.
        assert_eq!(
            html.matches("<script").count(),
            2,
            "ours plus the JSON island"
        );
        assert_eq!(html.matches("</script>").count(), 2);
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;/script&gt;&lt;img src=x onerror=alert(1)&gt;.txt"));
        assert!(html.contains("&quot;quote&quot;.txt"));
        assert!(html.contains("it&#39;s.txt"));
        assert!(html.contains(&"x".repeat(300)));

        // The JSON island holds the exact names and no raw < > & or line separators.
        let start = html.find("id=\"d\">").expect("island") + 7;
        let end = html[start..].find("</script>").expect("island end") + start;
        let island = &html[start..end];
        for bad in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(!island.contains(bad), "island contains {bad:?}");
        }
        let parsed: serde_json::Value = serde_json::from_str(island).expect("valid JSON");
        let names: Vec<&str> = parsed["names"]
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|name| name.as_str())
            .collect();
        for expected in HOSTILE {
            assert!(names.contains(expected), "lost {expected:?}");
        }

        // CSV: every name survives a round trip, formulas and tabs are defused.
        let csv = doc.csv();
        for expected in HOSTILE {
            let defused = if expected.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{expected}")
            } else {
                (*expected).to_string()
            };
            let cell = csv_cell(&defused);
            assert!(csv.contains(&cell), "csv lost {expected:?}");
        }
        assert!(!csv.contains("\n=") && !csv.contains(",=") && !csv.contains("\r\n="));
    }

    #[test]
    fn row_tones_details_and_code_blocks_render() {
        let mut doc = sample();
        doc.tables[0].row_tone = Some(vec![Tone::Good, Tone::Warn]);
        doc.sections = vec![
            Section::Details {
                summary: "Arrived (1)".into(),
                open: false,
                body: vec![Section::Note("fine".into())],
            },
            Section::Code(CodeBlock {
                label: "Script".into(),
                text: "robocopy 'a' 'b' </textarea>".into(),
                download_name: "fix.ps1".into(),
            }),
        ];
        let html = doc.html("Acme", "x");
        assert!(html.contains("<tr class=\"good\">") && html.contains("<tr class=\"warn\">"));
        assert!(html.contains("<details class=\"card fold\"><summary>Arrived (1)</summary>"));
        assert!(html.contains("&lt;/textarea&gt;"));
        assert!(html.contains("data-name=\"fix.ps1\""));
    }

    #[test]
    fn long_lists_are_capped_in_html_but_not_in_csv() {
        let rows: Vec<Vec<String>> = (0..MAX_HTML_ROWS + 3)
            .map(|index| vec![format!("f{index}")])
            .collect();
        let doc = Document {
            tables: vec![Table {
                heading: "Many".into(),
                columns: vec!["Path".into()],
                numeric: vec![false],
                rows,
                ..Table::default()
            }],
            ..Document::default()
        };
        let html = doc.html("Acme", "x");
        // One header row plus the capped body.
        assert_eq!(html.matches("<tr>").count(), MAX_HTML_ROWS + 1);
        assert!(html.contains("3 more not shown here"));
        assert!(html.contains("<table id=\"t1\" class=\"long\">"));
        assert_eq!(doc.csv().matches("\r\n").count(), MAX_HTML_ROWS + 3 + 1);
    }
}
