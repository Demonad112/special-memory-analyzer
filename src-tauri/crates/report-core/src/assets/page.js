
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
return L.join('\n')+'\n'};
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
a.href=URL.createObjectURL(new Blob(['\ufeff'+v.replace(/\r?\n/g,'\r\n')],{type:'text/plain'}));a.download=b.getAttribute('data-name');
document.body.appendChild(a);a.click();a.remove()})})}
})();
