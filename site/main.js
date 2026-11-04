const b=document.querySelector('.menu'),n=document.getElementById('links');
if(b&&n){b.addEventListener('click',()=>{const o=n.classList.toggle('open');b.setAttribute('aria-expanded',o)});
n.addEventListener('click',e=>{if(e.target.tagName==='A'){n.classList.remove('open');b.setAttribute('aria-expanded','false')}});}
// A copy button on every code block; copies the block's exact text.
document.querySelectorAll('pre').forEach(pre=>{
  const w=document.createElement('div');w.className='code';pre.parentNode.insertBefore(w,pre);w.appendChild(pre);
  pre.tabIndex=0;
  const c=document.createElement('button');c.type='button';c.className='copy';c.textContent='Copy';c.title='Copy to clipboard';c.setAttribute('aria-live','polite');
  const done=t=>{c.textContent=t;c.classList.toggle('done',t==='Copied');clearTimeout(c._t);c._t=setTimeout(()=>{c.textContent='Copy';c.classList.remove('done')},2000)};
  c.addEventListener('click',async()=>{const t=pre.innerText.replace(/\n$/,'');
    try{await navigator.clipboard.writeText(t);done('Copied')}catch(e){
      const r=document.createRange();r.selectNodeContents(pre);const s=getSelection();s.removeAllRanges();s.addRange(r);
      done(document.execCommand('copy')?'Copied':'Press Ctrl+C')}});
  w.appendChild(c);
});
