const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const {uriFrom}=require('../src/ui/playback-core.js');
function harness(seed={}){
  const storage=new Map(Object.entries(seed));
  let writes=0;const nodes={};
  function node(){const children={};let content='';return {dataset:{},value:0,className:'',append(){},remove(){},
    querySelector:s=>children[s] ||= node(),get textContent(){return content;},set textContent(v){content=v;writes++;}};}
  const context=vm.createContext({URLSearchParams,AbortSignal,SpotLiteCore:{uriFrom},Date:class extends Date{static now(){return 1000;}},
    document:{hidden:false,getElementById:id=>nodes[id] ||= node(),createElement:node,addEventListener(){}},
    localStorage:{getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,String(v)),removeItem:k=>storage.delete(k)},confirm:()=>true,setInterval(){},
    fetch:async()=>({ok:true,json:async()=>[],text:async()=>'deleted'})});
  vm.runInContext(fs.readFileSync('src/ui/control.js','utf8'),context);
  return {context,storage,get writes(){return writes;},eval:s=>vm.runInContext(s,context),tick:async()=>{for(let i=0;i<8;i++)await Promise.resolve();}};
}
test('autostart is opt-in and starts only closed saved accounts once per process boot',async()=>{
  const h=harness();await h.tick();const sent=[];
  h.storage.set('auto-start','1');h.storage.set('account-1',JSON.stringify({url:'spotify:track:7GqvrVU8R6tBv9DAN1USaG',mode:'track'}));
  h.context.fetch=async p=>{sent.push(p);return {ok:true,json:async()=>({boot:123}),text:async()=>'ok'};};
  h.eval('createRow(1);startupChecked=false');await h.eval('startup([{instance:1,running:false}])');
  h.eval('startupChecked=false');await h.eval('startup([{instance:1,running:false}])');
  assert.equal(sent.filter(p=>p.startsWith('/control-account')).length,1);
  h.storage.set('auto-boot','122');h.eval('startupChecked=false');await h.eval('startup([{instance:1,running:true}])');
  assert.equal(sent.filter(p=>p.startsWith('/control-account')).length,1);
});
test('corrupted health cache does not break panel and old account identity is hidden',async()=>{
  const h=harness({'health-1':'not json'});await h.tick();
  h.context.fetch=async()=>({ok:true,json:async()=>[{instance:1,name:'private identity',running:false}]});
  await h.eval('refresh()');assert.equal(h.eval('rows.get(1).name.textContent'),'Hesap 1');
});
test('unchanged playback does not rewrite status DOM',async()=>{
  const h=harness();await h.tick();h.eval("globalThis.row=createRow(1);row.running=true;row.runtime={updated:1000,state:'paused',position:1000,duration:5000};paintRuntime(row)");
  const before=h.writes;h.eval('paintRuntime(row)');assert.equal(h.writes,before);
});
test('deleted account number can be displayed again when reused',async()=>{
  const h=harness();await h.tick();h.eval('createRow(1)');await h.eval("rows.get(1).el.querySelector('.danger').onclick()");await h.tick();
  assert.equal(h.eval('deleted.has(1)'),false);assert.equal(h.eval('rows.has(1)'),false);
  h.context.fetch=async()=>({ok:true,json:async()=>[{instance:1,name:'New account',running:false,auth:{}}]});await h.eval('refresh()');
  assert.equal(h.eval('rows.get(1).name.textContent'),'Hesap 1');
});
test('account response from before a deletion cannot resurrect old data',async()=>{
  const h=harness();await h.tick();let resolve;
  h.context.fetch=async()=>({ok:true,json:()=>new Promise(r=>resolve=r)});
  const pending=h.eval('refresh()');await h.tick();h.eval('revision++');resolve([{instance:1,name:'Old'}]);await pending;
  assert.equal(h.eval('rows.size'),0);
});
