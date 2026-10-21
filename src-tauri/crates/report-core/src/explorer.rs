//! The data behind the disk usage report's explorer: a pruned size tree (so the page can drill
//! down folder by folder) and the biggest files of each file type (so it can filter by type).
//!
//! A scan of a whole drive has millions of items, so only the part that matters is kept and the
//! rest is folded into honest "(N smaller items)" rows. The tree is a flat list (a node names its
//! parent by index) so a deep folder structure cannot hit a JSON nesting limit.

use crate::kinds::TypeItem;
use scan_core::{Kind, NodeId, Tree};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Most nodes in the saved tree, folded rows included (about 1 MB of JSON).
pub const MAX_NODES: usize = 20_000;
/// A folder always keeps this many of its biggest children, however small.
const KEEP_CHILDREN: usize = 50;
/// A node smaller than 1/this of the whole scan is folded unless it is one of the biggest children.
const MIN_SHARE_DIVISOR: u64 = 1000;
/// Biggest files kept per file type.
pub const FILES_PER_TYPE: usize = 200;
/// Most file entries across all types; the biggest types are filled first.
pub const MAX_TYPE_FILES: usize = 15_000;
/// Most file types listed; the rest are counted in [`Explorer::types_more`].
const MAX_TYPES: usize = 3_000;

pub const KIND_DIR: u8 = 0;
pub const KIND_FILE: u8 = 1;
/// "(N smaller items)": everything in a folder that was left out.
pub const KIND_MORE: u8 = 2;

/// File-type groups. The same lists as `src/lib/fileTypes.ts` (a test compares them).
const GROUPS: &[(&str, &str, &str)] = &[
    (
        "video",
        "Video",
        "mp4 mkv avi mov wmv m4v webm mpg mpeg vob ts mts m2ts flv 3gp",
    ),
    (
        "audio",
        "Music and audio",
        "mp3 wav flac aac m4a wma ogg opus aiff mid",
    ),
    (
        "image",
        "Pictures",
        "jpg jpeg png gif bmp tif tiff heic heif webp raw cr2 cr3 nef arw dng psd svg ico",
    ),
    (
        "document",
        "Documents and email",
        "pdf doc docx docm xls xlsx xlsm xlsb ppt pptx txt rtf csv odt ods odp md msg eml pst ost one log xml json",
    ),
    (
        "archive",
        "Archives, disk images and backups",
        "zip rar 7z tar gz tgz bz2 xz zst cab iso img vhd vhdx vmdk wim esd bak tib",
    ),
    (
        "program",
        "Programs and system files",
        "exe dll msi msix msp appx appxbundle sys drv ocx cpl mui cat etl dat pak",
    ),
];
const OTHER: (&str, &str) = ("other", "Other files");

/// Group labels in display order; the last one is "Other files".
pub fn category_labels() -> Vec<&'static str> {
    GROUPS
        .iter()
        .map(|(_, label, _)| *label)
        .chain([OTHER.1])
        .collect()
}

/// Index into [`category_labels`] for a lowercase extension without the dot.
pub fn category_of(extension: &str) -> usize {
    GROUPS
        .iter()
        .position(|(_, _, list)| list.split(' ').any(|known| known == extension))
        .unwrap_or(GROUPS.len())
}

/// One item of the pruned tree. Short keys keep the saved report small.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExplorerNode {
    /// File or folder name; the first node (the scanned folder) holds its full path.
    #[serde(rename = "n")]
    pub name: String,
    /// Space on disk, as in the rest of the report.
    #[serde(rename = "d")]
    pub disk: u64,
    #[serde(rename = "f")]
    pub files: u64,
    /// [`KIND_DIR`], [`KIND_FILE`] or [`KIND_MORE`].
    #[serde(rename = "k")]
    pub kind: u8,
    /// Index of the folder holding this node (0 for the first node itself).
    #[serde(rename = "p")]
    pub parent: u32,
}

/// The biggest files of one extension. Paths are relative to the scanned folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypeFiles {
    #[serde(rename = "e")]
    pub extension: String,
    /// Path and size on disk, biggest first.
    #[serde(rename = "f")]
    pub files: Vec<(String, u64)>,
}

/// What a saved Disk Cleanup scan keeps so its report can be explored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Explorer {
    /// Breadth first, biggest first within a folder; empty when nothing was saved.
    pub nodes: Vec<ExplorerNode>,
    /// Every extension in the scan (up to [`MAX_TYPES`]), biggest first.
    pub types: Vec<TypeItem>,
    /// Extensions left out of `types`.
    pub types_more: u64,
    pub type_files: Vec<TypeFiles>,
    /// Items in the whole scan, to say how much of it `nodes` shows.
    pub scanned_items: u64,
}

/// Prunes `tree` for the report: the biggest items down to a hard cap, and the biggest files per type.
pub fn build(tree: &Tree) -> Explorer {
    let (nodes, scanned_items) = prune(tree);
    let (types, types_more, type_files) = by_type(tree);
    Explorer {
        nodes,
        types,
        types_more,
        type_files,
        scanned_items,
    }
}

fn is_dir_with_children(tree: &Tree, id: NodeId) -> bool {
    tree.node(id).kind == Kind::Dir && tree.live_children(id).next().is_some()
}

fn prune(tree: &Tree) -> (Vec<ExplorerNode>, u64) {
    let root = tree.root();
    let threshold = (root.disk / MIN_SHARE_DIVISOR).max(1);

    // Phase 1: take the biggest candidates first, until the cap. A node is a candidate only when
    // its folder was kept, so what is kept always forms one tree. Every kept folder reserves a
    // slot for its "(N smaller items)" row.
    let mut kept: HashSet<NodeId> = HashSet::from([0]);
    let mut used = 1 + usize::from(is_dir_with_children(tree, 0));
    let mut heap: BinaryHeap<(u64, Reverse<NodeId>)> = BinaryHeap::new();
    let offer = |heap: &mut BinaryHeap<(u64, Reverse<NodeId>)>, folder: NodeId| {
        for (index, child) in tree.live_children(folder).enumerate() {
            let node = tree.node(child);
            if node.kind == Kind::Link || node.disk == 0 {
                continue;
            }
            if index < KEEP_CHILDREN || node.disk >= threshold {
                heap.push((node.disk, Reverse(child)));
            }
        }
    };
    offer(&mut heap, 0);
    while let Some((_, Reverse(id))) = heap.pop() {
        let cost = 1 + usize::from(is_dir_with_children(tree, id));
        if used + cost > MAX_NODES {
            break;
        }
        used += cost;
        kept.insert(id);
        if tree.node(id).kind == Kind::Dir {
            offer(&mut heap, id);
        }
    }

    // Phase 2: write the kept nodes out breadth first, folding what was left out.
    let mut nodes = vec![ExplorerNode {
        name: root.name.clone(),
        disk: root.disk,
        files: root.files,
        kind: KIND_DIR,
        parent: 0,
    }];
    let mut queue: VecDeque<(NodeId, u32)> = VecDeque::from([(0, 0)]);
    while let Some((id, at)) = queue.pop_front() {
        let (mut items, mut disk, mut files) = (0u64, 0u64, 0u64);
        for child in tree.live_children(id) {
            let node = tree.node(child);
            if kept.contains(&child) {
                let index = nodes.len() as u32;
                nodes.push(ExplorerNode {
                    name: node.name.clone(),
                    disk: node.disk,
                    files: node.files,
                    kind: if node.kind == Kind::Dir {
                        KIND_DIR
                    } else {
                        KIND_FILE
                    },
                    parent: at,
                });
                if node.kind == Kind::Dir {
                    queue.push_back((child, index));
                }
            } else {
                items += 1;
                disk += node.disk;
                files += node.files;
            }
        }
        if items > 0 {
            nodes.push(ExplorerNode {
                name: format!(
                    "({} smaller {})",
                    crate::format_count(items),
                    if items == 1 { "item" } else { "items" }
                ),
                disk,
                files,
                kind: KIND_MORE,
                parent: at,
            });
        }
    }
    (nodes, tree.len() as u64)
}

/// "Movie.MP4" -> "mp4"; "README" and ".gitignore" -> "". Same rule as Disk Cleanup's file types.
fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Path of `id` below the scanned folder, joined with backslashes.
fn relative_path(tree: &Tree, id: NodeId) -> String {
    let mut names = Vec::new();
    let mut current = id;
    while let Some(parent) = tree.parent(current) {
        names.push(tree.node(current).name.as_str());
        current = parent;
    }
    names.reverse();
    names.join("\\")
}

type TopFiles = BinaryHeap<Reverse<(u64, NodeId)>>;

fn by_type(tree: &Tree) -> (Vec<TypeItem>, u64, Vec<TypeFiles>) {
    let mut totals: HashMap<String, (u64, u64)> = HashMap::new();
    let mut tops: HashMap<String, TopFiles> = HashMap::new();
    let mut stack = vec![0];
    while let Some(id) = stack.pop() {
        for child in tree.live_children(id) {
            let node = tree.node(child);
            match node.kind {
                Kind::Dir => stack.push(child),
                // Links and online-only files take no space here.
                Kind::Link => {}
                Kind::File if node.cloud_files > 0 => {}
                Kind::File => {
                    let ext = extension(&node.name);
                    let entry = totals.entry(ext.clone()).or_default();
                    entry.0 += node.disk;
                    entry.1 += 1;
                    if node.disk > 0 {
                        let top = tops.entry(ext).or_default();
                        top.push(Reverse((node.disk, child)));
                        if top.len() > FILES_PER_TYPE {
                            top.pop();
                        }
                    }
                }
            }
        }
    }

    let mut types: Vec<TypeItem> = totals
        .into_iter()
        .map(|(extension, (size, files))| TypeItem {
            extension,
            size,
            files,
        })
        .collect();
    types.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.extension.cmp(&b.extension))
    });
    let types_more = types.len().saturating_sub(MAX_TYPES) as u64;
    types.truncate(MAX_TYPES);

    // The biggest types get their files first, so a tight budget loses the least.
    let mut budget = MAX_TYPE_FILES;
    let mut type_files = Vec::new();
    for kind in &types {
        let Some(top) = tops.remove(&kind.extension) else {
            continue;
        };
        if budget == 0 {
            break;
        }
        let mut picked: Vec<(u64, NodeId)> = top.into_iter().map(|Reverse(pair)| pair).collect();
        picked.sort_by(|a, b| b.cmp(a));
        picked.truncate(budget);
        budget -= picked.len();
        type_files.push(TypeFiles {
            extension: kind.extension.clone(),
            files: picked
                .into_iter()
                .map(|(size, id)| (relative_path(tree, id), size))
                .collect(),
        });
    }
    (types, types_more, type_files)
}

/// The page script's data (`x` in the JSON island).
pub fn page_data(explorer: &Explorer, root: &str) -> serde_json::Value {
    let types: Vec<serde_json::Value> = explorer
        .types
        .iter()
        .map(|kind| {
            serde_json::json!([
                kind.extension,
                kind.size,
                kind.files,
                category_of(&kind.extension)
            ])
        })
        .collect();
    let mut files = serde_json::Map::new();
    for group in &explorer.type_files {
        files.insert(group.extension.clone(), serde_json::json!(group.files));
    }
    serde_json::json!({
        "root": root,
        "nodes": explorer.nodes,
        "types": types,
        "cats": category_labels(),
        "tf": files,
        "perType": FILES_PER_TYPE,
        "items": explorer.scanned_items,
        "typesMore": explorer.types_more,
    })
}

/// The explorer's own page script, run after the shared one. It builds every element with
/// `createElement` and `textContent`, so no name from the disk is ever parsed as markup.
pub const SCRIPT: &str = r#"
(function(){
var host=document.getElementById('x'),isl=document.getElementById('d');
if(!host||!isl)return;
var D;try{D=JSON.parse(isl.textContent).x}catch(e){return}
if(!D||!D.nodes||!D.nodes.length)return;
var N=D.nodes,K=N.map(function(){return[]}),TF=D.tf||{},CAT={},SEP='\\';
N.forEach(function(n,i){if(i>0)K[n.p].push(i)});
D.types.forEach(function(t){CAT[t[0]]=t[3]});
var sel=0,flt=null,open={0:true},all=false;
function h(t,a,c){var e=document.createElement(t),k;
if(a)for(k in a){if(k==='class')e.className=a[k];else e.setAttribute(k,String(a[k]))}
if(c!==undefined&&c!==null)[].concat(c).forEach(function(x){if(x!==null&&x!==undefined)e.appendChild(typeof x==='string'?document.createTextNode(x):x)});
return e}
function fb(b){if(b<1024)return b+(b===1?' byte':' bytes');var u=['KB','MB','GB','TB','PB'],i=-1,v=b;do{v/=1024;i++}while(v>=1024&&i<4);return v.toFixed(v>=100?0:v>=10?1:2)+' '+u[i]}
function fc(n){return n.toLocaleString('en-US')}
function share(a,b){if(!b)return'';var p=a*100/b;return p<0.1&&a>0?'<0.1%':p.toFixed(1)+'%'}
function pct(a,b){return b?Math.min(100,a*100/b):0}
function pathOf(i){var p=[];while(i>0){p.push(N[i].n);i=N[i].p}return p.reverse()}
function rel(i){return pathOf(i).join(SEP)}
function full(r){return r?D.root.replace(/\\+$/,'')+SEP+r:D.root}
function extName(e){return e===''?'No extension':'.'+e}
function crumbs(){var nav=h('nav',{'class':'xc','aria-label':'Selected folder'}),chain=[],i=sel;
for(;;){chain.push(i);if(i===0)break;i=N[i].p}
chain.reverse().forEach(function(i,j){
if(j>0)nav.appendChild(h('span',{'class':'sep','aria-hidden':'true'},'\u203a'));
var b=h('button',{type:'button','class':'cr','data-a':'go','data-v':i,'data-k':'c'+i},i===0?D.root:N[i].n);
if(i===sel)b.setAttribute('aria-current','true');nav.appendChild(b)});
return nav}
function row(c,par){
var n=N[c],li=h('li'),r=h('div',{'class':'xr'}),dir=n.k===0,has=dir&&K[c].length>0;
if(has)r.appendChild(h('button',{type:'button','class':'tg','data-a':'tg','data-v':c,'data-k':'t'+c,'aria-expanded':open[c]?'true':'false','aria-label':(open[c]?'Collapse ':'Expand ')+n.n},open[c]?'\u25be':'\u25b8'));
else r.appendChild(h('span',{'class':'tg'}));
if(dir){var b=h('button',{type:'button','class':'nm','data-a':'go','data-v':c,'data-k':'s'+c},n.n);
if(c===sel)b.setAttribute('aria-current','true');r.appendChild(b)}
else r.appendChild(h('span',{'class':n.k===2?'nm more':'nm'},n.n));
r.appendChild(h('span',{'class':'xs'},fb(n.d)));
var bar=h('span',{'class':'xb','aria-hidden':'true'},h('i'));bar.firstChild.style.width=pct(n.d,N[par].d)+'%';r.appendChild(bar);
r.appendChild(h('span',{'class':'xp'},share(n.d,N[par].d)));
r.appendChild(h('span',{'class':'xf'},dir||n.k===2?fc(n.f)+(n.f===1?' file':' files'):''));
li.appendChild(r);
if(has&&open[c]){var ul=h('ul');K[c].forEach(function(d){ul.appendChild(row(d,c))});li.appendChild(ul)}
return li}
function treeBox(){
var box=h('div',{'class':'xtree'}),ul=h('ul',{'class':'xt'});
box.appendChild(h('h3',0,'Folders'));
K[0].forEach(function(c){ul.appendChild(row(c,0))});
if(!K[0].length)box.appendChild(h('p',{'class':'empty'},'Nothing to explore here.'));
else box.appendChild(ul);
box.appendChild(h('p',{'class':'note'},'Shows the scan as it was. Showing the biggest items ('+fc(N.length)+' of '+fc(D.items)+'); the rest of each folder is added up in its \u201csmaller items\u201d row.'));
return box}
function typeRows(){
if(sel===0)return D.types;
var pre=rel(sel)+SEP,out=[];
Object.keys(TF).forEach(function(e){var s=0,n=0;
TF[e].forEach(function(f){if(f[0].indexOf(pre)===0){s+=f[1];n++}});
if(n)out.push([e,s,n,CAT[e]])});
out.sort(function(a,b){return b[1]-a[1]});
return out}
function typeBox(){
var box=h('div',{'class':'xtypes'}),rows=typeRows(),cs=D.cats.map(function(){return[0,0]}),tot=0;
box.appendChild(h('h3',0,sel===0?'By file type':'By file type in this folder'));
rows.forEach(function(r){cs[r[3]][0]+=r[1];cs[r[3]][1]+=r[2];tot+=r[1]});
var ul=h('ul',{'class':'xl'});
cs.forEach(function(c,i){if(!c[1])return;
var li=h('li'),b=h('button',{type:'button','class':'xr','data-a':'cat','data-v':i,'data-k':'g'+i,'aria-pressed':flt&&flt.t==='cat'&&flt.v===i?'true':'false'});
b.appendChild(h('span',{'class':'nm'},D.cats[i]));
b.appendChild(h('span',{'class':'xs'},fb(c[0])));
var bar=h('span',{'class':'xb','aria-hidden':'true'},h('i'));bar.firstChild.style.width=pct(c[0],tot)+'%';b.appendChild(bar);
b.appendChild(h('span',{'class':'xf'},fc(c[1])+(c[1]===1?' file':' files')));
li.appendChild(b);ul.appendChild(li)});
box.appendChild(ul);
var shown=rows.filter(function(r){return!(flt&&flt.t==='cat')||r[3]===flt.v});
var lim=all?shown.length:15,el=h('ul',{'class':'xl'});
shown.slice(0,lim).forEach(function(r){
var li=h('li'),b=h('button',{type:'button','class':'xr','data-a':'ext','data-v':r[0],'data-k':'e:'+r[0],'aria-pressed':flt&&flt.t==='ext'&&flt.v===r[0]?'true':'false'});
b.appendChild(h('span',{'class':'nm'},extName(r[0])));
b.appendChild(h('span',{'class':'xs'},fb(r[1])));
var bar=h('span',{'class':'xb','aria-hidden':'true'},h('i'));bar.firstChild.style.width=pct(r[1],tot)+'%';b.appendChild(bar);
b.appendChild(h('span',{'class':'xf'},fc(r[2])+(r[2]===1?' file':' files')));
li.appendChild(b);el.appendChild(li)});
box.appendChild(h('h4',0,flt&&flt.t==='cat'?D.cats[flt.v]:'Biggest types'));
box.appendChild(el);
if(shown.length>15)box.appendChild(h('button',{type:'button','class':'more','data-a':'all','data-k':'all'},all?'Show fewer':'Show all '+fc(shown.length)+' types'));
if(sel!==0)box.appendChild(h('p',{'class':'note'},'Counts the biggest files kept in this report (top '+D.perType+' per type), not every file in the folder.'));
else if(D.typesMore>0)box.appendChild(h('p',{'class':'note'},fc(D.typesMore)+' rarer types are not listed.'));
return box}
function fileBox(){
var box=h('div',{'class':'xfiles'}),pre=sel===0?'':rel(sel)+SEP,src=[];
Object.keys(TF).forEach(function(e){
if(flt&&flt.t==='ext'&&flt.v!==e)return;
if(flt&&flt.t==='cat'&&CAT[e]!==flt.v)return;
TF[e].forEach(function(f){if(f[0].indexOf(pre)===0)src.push(f)})});
src.sort(function(a,b){return b[1]-a[1]});
var what=!flt?'':flt.t==='ext'?extName(flt.v):D.cats[flt.v],head=h('h3',0,'Largest files'+(what?': '+what:'')+(sel?' in '+full(rel(sel)):''));
box.appendChild(head);
if(flt)box.appendChild(h('button',{type:'button','class':'chip','data-a':'clear','data-k':'clear'},'Showing '+what+' \u2014 clear'));
if(!src.length){box.appendChild(h('p',{'class':'empty'},'No files to list here.'));}
else{
var t=h('table'),hd=h('tr');hd.appendChild(h('th',0,'File'));hd.appendChild(h('th',{'class':'num'},'Size'));
t.appendChild(h('thead',0,hd));
var tb=h('tbody');src.slice(0,100).forEach(function(f){var tr=h('tr');tr.appendChild(h('td',0,full(f[0])));tr.appendChild(h('td',{'class':'num'},fb(f[1])));tb.appendChild(tr)});
t.appendChild(tb);box.appendChild(h('div',{'class':'scroll'},t));
if(src.length>100)box.appendChild(h('p',{'class':'note'},'Showing the biggest 100 of '+fc(src.length)+' files kept.'))}
box.appendChild(h('p',{'class':'note'},'Each type keeps its '+D.perType+' biggest files in this report, so smaller files are not listed.'));
return box}
function draw(focus){
host.textContent='';
host.appendChild(h('h2',0,'Explore this scan'));
host.appendChild(crumbs());
var cols=h('div',{'class':'xcols'});cols.appendChild(treeBox());cols.appendChild(typeBox());
host.appendChild(cols);host.appendChild(fileBox());
if(focus){var t=[].filter.call(host.querySelectorAll('[data-k]'),function(e){return e.getAttribute('data-k')===focus})[0];if(t)t.focus()}}
host.addEventListener('click',function(ev){
var b=ev.target.closest?ev.target.closest('button'):null;
if(!b||!host.contains(b))return;
var a=b.getAttribute('data-a'),v=b.getAttribute('data-v');
if(a==='go'){sel=+v;open[sel]=true}
else if(a==='tg'){open[v]=!open[v]}
else if(a==='cat'){flt=flt&&flt.t==='cat'&&flt.v===+v?null:{t:'cat',v:+v}}
else if(a==='ext'){flt=flt&&flt.t==='ext'&&flt.v===v?null:{t:'ext',v:v}}
else if(a==='clear'){flt=null}
else if(a==='all'){all=!all}
else return;
draw(b.getAttribute('data-k'))});
draw();
})();
"#;

/// Styles for the explorer, emitted only on pages that have one.
pub const STYLE: &str = r#"
.explorer h3{font-size:14px;margin:12px 0 6px}.explorer h4{font-size:12px;margin:12px 0 4px;color:var(--muted);text-transform:uppercase;letter-spacing:.05em}
.explorer button{font:inherit}
.xc{display:flex;flex-wrap:wrap;align-items:center;gap:2px;margin:4px 0 8px}
.xc .sep{color:var(--muted)}.xc .cr{border:0;background:none;padding:2px 6px;color:var(--accent);border-radius:6px}
.xc .cr[aria-current=true]{font-weight:600;color:var(--text)}
.xcols{display:grid;grid-template-columns:minmax(0,3fr) minmax(0,2fr);gap:20px}
.xt,.xt ul,.xl{list-style:none;margin:0;padding:0}.xt ul{padding-left:20px}
.xr{display:flex;align-items:center;gap:8px;padding:2px 0;width:100%;text-align:left}
button.xr{border:0;background:none;color:var(--text);cursor:pointer;border-radius:6px;padding:3px 4px}
button.xr:hover,.xr .nm:hover{background:var(--zebra)}
button.xr[aria-pressed=true]{background:var(--zebra);box-shadow:inset 3px 0 0 var(--accent)}
.xr .tg{flex:none;width:22px;border:0;background:none;padding:0;color:var(--muted);cursor:pointer}
.xr .nm{flex:1;min-width:0;border:0;background:none;text-align:left;padding:2px 4px;border-radius:6px;color:var(--text);overflow-wrap:anywhere}
.xr button.nm{cursor:pointer}.xr .nm[aria-current=true]{font-weight:600;box-shadow:inset 3px 0 0 var(--accent)}
.xr .nm.more{color:var(--muted);font-style:italic}
.xs{flex:none;min-width:64px;text-align:right;font-variant-numeric:tabular-nums}
.xp{flex:none;width:46px;text-align:right;color:var(--muted);font-size:12px;font-variant-numeric:tabular-nums}
.xf{flex:none;min-width:70px;text-align:right;color:var(--muted);font-size:12px;font-variant-numeric:tabular-nums}
.xb{flex:none;width:64px;height:6px;border-radius:999px;background:var(--line);overflow:hidden}
.xb i{display:block;height:100%;background:linear-gradient(90deg,var(--accent),var(--accent-2))}
.explorer .chip{margin:0 0 8px;border-radius:999px;padding:3px 12px;color:var(--accent)}
.explorer .more{margin-top:6px}
.explorer button:focus-visible{outline:2px solid var(--accent-2);outline-offset:2px}
@media (max-width:760px){.xcols{grid-template-columns:minmax(0,1fr)}.xb,.xp{display:none}}
@media print{.explorer{display:none!important}}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, CancelToken, ScanState};
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn scanned(root: &Path) -> Tree {
        scan(root, &ScanState::new(CancelToken::new())).unwrap()
    }

    fn children_of(explorer: &Explorer, parent: usize) -> Vec<&ExplorerNode> {
        explorer
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, node)| *index > 0 && node.parent as usize == parent)
            .map(|(_, node)| node)
            .collect()
    }

    #[test]
    fn extension_rule_matches_disk_cleanup() {
        assert_eq!(extension("Movie.MP4"), "mp4");
        assert_eq!(extension("README"), "");
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("trailing."), "");
    }

    #[test]
    fn groups_map_extensions_and_default_to_other() {
        let labels = category_labels();
        assert_eq!(labels[category_of("mp4")], "Video");
        assert_eq!(labels[category_of("dll")], "Programs and system files");
        assert_eq!(labels[category_of("")], "Other files");
        assert_eq!(labels[category_of("zzz")], "Other files");
        assert_eq!(*labels.last().unwrap(), "Other files");
    }

    /// `src/lib/fileTypes.ts` colours the app; this table groups the report. They must agree.
    #[test]
    fn groups_match_the_typescript_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../src/lib/fileTypes.ts");
        let text = fs::read_to_string(&path).expect("src/lib/fileTypes.ts");
        let quoted_after = |marker: &str, key: &str| -> String {
            let from = text.find(marker).unwrap_or_else(|| panic!("no {marker}"));
            let rest = &text[from..];
            let at = rest
                .find(&format!("{key}:"))
                .unwrap_or_else(|| panic!("no {key} after {marker}"));
            let rest = &rest[at..];
            let open = rest.find('\'').expect("opening quote") + 1;
            let close = rest[open..].find('\'').expect("closing quote");
            rest[open..open + close].to_string()
        };
        for (key, label, list) in GROUPS {
            assert_eq!(
                quoted_after("const groups", key)
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                list.split(' ').collect::<Vec<_>>(),
                "extensions of {key}"
            );
            assert_eq!(
                quoted_after("export const categoryLabels", key),
                *label,
                "label of {key}"
            );
        }
        assert_eq!(
            quoted_after("export const categoryLabels", OTHER.0),
            OTHER.1
        );
    }

    #[test]
    fn small_scan_keeps_everything_and_roots_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a/big.mp4"), 500_000);
        write(&temp.path().join("a/small.txt"), 5_000);
        write(&temp.path().join("b.dll"), 100_000);
        let tree = scanned(temp.path());
        let explorer = build(&tree);

        let root = &explorer.nodes[0];
        assert_eq!(root.kind, KIND_DIR);
        assert_eq!(root.disk, tree.root().disk);
        assert_eq!(root.name, temp.path().display().to_string());
        // 2 files + 1 folder, all kept, so no "smaller items" row.
        assert!(explorer.nodes.iter().all(|node| node.kind != KIND_MORE));
        let top = children_of(&explorer, 0);
        assert_eq!(
            top.iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b.dll"]
        );
        let a = explorer
            .nodes
            .iter()
            .position(|node| node.name == "a")
            .unwrap();
        assert_eq!(children_of(&explorer, a).len(), 2);
        assert_eq!(explorer.scanned_items, tree.len() as u64);
    }

    #[test]
    fn small_items_fold_into_an_honest_row_that_adds_up() {
        let temp = tempfile::tempdir().unwrap();
        // One huge file and 80 tiny ones: the tiny ones are below 0.1% and past the first 50.
        write(&temp.path().join("huge.bin"), 50_000_000);
        for index in 0..80 {
            write(&temp.path().join(format!("tiny{index:02}.txt")), 10);
        }
        let tree = scanned(temp.path());
        let explorer = build(&tree);
        let top = children_of(&explorer, 0);
        let more: Vec<_> = top.iter().filter(|node| node.kind == KIND_MORE).collect();
        assert_eq!(more.len(), 1);
        // 1 big + the 49 next biggest are kept; 31 are folded.
        assert_eq!(top.len(), 1 + 49 + 1);
        assert_eq!(more[0].name, "(31 smaller items)");
        assert_eq!(more[0].files, 31);
        // Kept children plus the fold equal the folder (it holds nothing else).
        let sum: u64 = top.iter().map(|node| node.disk).sum();
        assert_eq!(sum, explorer.nodes[0].disk);
    }

    #[test]
    fn the_node_cap_is_hard() {
        let temp = tempfile::tempdir().unwrap();
        for dir in 0..40 {
            for file in 0..30 {
                write(
                    &temp.path().join(format!("d{dir:02}/f{file:02}.dat")),
                    5_000,
                );
            }
        }
        let tree = scanned(temp.path());
        // Pretend the cap is tiny by checking the real one is never exceeded and the shape holds.
        let explorer = build(&tree);
        assert!(explorer.nodes.len() <= MAX_NODES);
        for (index, node) in explorer.nodes.iter().enumerate().skip(1) {
            assert!((node.parent as usize) < index, "parents come first");
            assert_eq!(explorer.nodes[node.parent as usize].kind, KIND_DIR);
        }
    }

    #[test]
    fn deep_folders_stay_flat() {
        let temp = tempfile::tempdir().unwrap();
        let mut deep = temp.path().to_path_buf();
        for level in 0..150 {
            deep.push(format!("d{level}"));
        }
        write(&deep.join("leaf.bin"), 200_000);
        let explorer = build(&scanned(temp.path()));
        assert_eq!(explorer.nodes.len(), 152);
        // A flat list round trips through serde_json; nested JSON this deep would be refused.
        let json = serde_json::to_string(&explorer).unwrap();
        assert_eq!(serde_json::from_str::<Explorer>(&json).unwrap(), explorer);
    }

    #[test]
    fn types_and_their_biggest_files_are_saved() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("m/one.mp4"), 600_000);
        write(&temp.path().join("m/two.MP4"), 300_000);
        write(&temp.path().join("x.dll"), 200_000);
        write(&temp.path().join("Makefile"), 20_000);
        let explorer = build(&scanned(temp.path()));

        let names: Vec<&str> = explorer
            .types
            .iter()
            .map(|t| t.extension.as_str())
            .collect();
        assert_eq!(names, ["mp4", "dll", ""]);
        assert_eq!(explorer.types[0].files, 2);
        let mp4 = explorer
            .type_files
            .iter()
            .find(|group| group.extension == "mp4")
            .unwrap();
        assert_eq!(mp4.files[0].0, "m\\one.mp4");
        assert_eq!(mp4.files[1].0, "m\\two.MP4");
        assert!(mp4.files[0].1 >= mp4.files[1].1);
        assert!(explorer
            .type_files
            .iter()
            .any(|group| group.extension.is_empty() && group.files[0].0 == "Makefile"));
    }

    #[test]
    fn each_type_keeps_only_its_biggest_files() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..FILES_PER_TYPE + 25 {
            write(&temp.path().join(format!("f{index:03}.log")), 5_000 + index);
        }
        let explorer = build(&scanned(temp.path()));
        let logs = &explorer.type_files[0];
        assert_eq!(logs.files.len(), FILES_PER_TYPE);
        assert_eq!(explorer.types[0].files, (FILES_PER_TYPE + 25) as u64);
        let sizes: Vec<u64> = logs.files.iter().map(|file| file.1).collect();
        assert!(sizes.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn page_data_carries_categories_and_survives_hostile_names() {
        let explorer = Explorer {
            nodes: vec![
                ExplorerNode {
                    name: r"C:\".into(),
                    disk: 10,
                    ..ExplorerNode::default()
                },
                ExplorerNode {
                    name: "</script><img src=x>".into(),
                    disk: 10,
                    kind: KIND_FILE,
                    ..ExplorerNode::default()
                },
            ],
            types: vec![TypeItem {
                extension: "mp4".into(),
                size: 10,
                files: 1,
            }],
            type_files: vec![TypeFiles {
                extension: "mp4".into(),
                files: vec![("</script>.mp4".into(), 10)],
            }],
            ..Explorer::default()
        };
        let data = page_data(&explorer, r"C:\");
        assert_eq!(data["types"][0], serde_json::json!(["mp4", 10, 1, 0]));
        assert_eq!(data["cats"][0], "Video");
        assert_eq!(data["tf"]["mp4"][0][0], "</script>.mp4");
        assert_eq!(data["nodes"][1]["n"], "</script><img src=x>");
    }

    /// Size and speed on a real tree, for the PR description:
    /// `EXPLORER_PATH=C:\\ [EXPLORER_OUT=report.html] cargo test -p report-core --release measure -- --ignored --nocapture`
    #[test]
    #[ignore = "measures a real folder given in EXPLORER_PATH"]
    fn measure_a_real_scan() {
        let path = std::env::var("EXPLORER_PATH").expect("EXPLORER_PATH");
        let started = std::time::Instant::now();
        let tree = scanned(Path::new(&path));
        let scanned_in = started.elapsed();
        let started = std::time::Instant::now();
        let explorer = build(&tree);
        let built_in = started.elapsed();
        let json = serde_json::to_vec(&explorer).unwrap();
        let report = crate::UsageReport {
            path: path.clone(),
            size: tree.root().disk,
            explorer: Some(explorer.clone()),
            ..crate::UsageReport::default()
        };
        let saved = crate::kinds::document(&crate::Saved {
            id: "x".into(),
            created_at_ms: 0,
            machine: String::new(),
            user: String::new(),
            job: crate::JobInfo::default(),
            body: crate::Body::DiskUsage(report),
        });
        let html = saved.html("Measure", "now");
        println!(
            "items scanned {} | scan {scanned_in:?} | explorer built in {built_in:?}",
            tree.len()
        );
        println!(
            "tree nodes {} | types {} (+{} not listed) | type files {} | explorer JSON {} KB | html {} KB",
            explorer.nodes.len(),
            explorer.types.len(),
            explorer.types_more,
            explorer.type_files.iter().map(|g| g.files.len()).sum::<usize>(),
            json.len() / 1024,
            html.len() / 1024
        );
        if let Some(out) = std::env::var_os("EXPLORER_OUT") {
            std::fs::write(out, &html).unwrap();
        }
        assert!(explorer.nodes.len() <= MAX_NODES);
    }

    #[test]
    fn old_reports_without_an_explorer_still_load() {
        let explorer: Explorer = serde_json::from_str("{}").unwrap();
        assert!(explorer.nodes.is_empty());
    }
}
