
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
function pathOf(i){var p=[],seen={};while(i>0&&!seen[i]){seen[i]=1;p.push(N[i].n);i=N[i].p}return p.reverse()}
function rel(i){return pathOf(i).join(SEP)}
function full(r){return r?D.root.replace(/\\+$/,'')+SEP+r:D.root}
function extName(e){return e===''?'No extension':'.'+e}
function crumbs(){var nav=h('nav',{'class':'xc','aria-label':'Selected folder'}),chain=[],seen={},i=sel;
while(!seen[i]){seen[i]=1;chain.push(i);if(i===0)break;i=N[i].p}
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
