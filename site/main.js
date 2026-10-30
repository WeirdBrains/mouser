const b=document.querySelector('.menu'),n=document.getElementById('links');
b.addEventListener('click',()=>{const o=n.classList.toggle('open');b.setAttribute('aria-expanded',o)});
n.addEventListener('click',e=>{if(e.target.tagName==='A'){n.classList.remove('open');b.setAttribute('aria-expanded','false')}});
