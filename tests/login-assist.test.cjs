const {test}=require('node:test');
const assert=require('node:assert/strict');
const vm=require('node:vm');
const fs=require('node:fs');
function page({origin='https://accounts.spotify.com',path='/tr/login',body='',email=false,password=false,button='Devam',iframe='',otp=false}={}) {
  let clicks=0,writes=0;
  class Input {set value(v){this.stored=v;writes++;}get value(){return this.stored;}getClientRects(){return [1];}dispatchEvent(){}}
  const e=email?new Input():null,p=password?new Input():null;
  const b={textContent:button,disabled:false,isConnected:true,getClientRects:()=>[1],click(){clicks++;}};
  const context={location:{origin,pathname:path},document:{body:{innerText:body},querySelectorAll(s){
    if(s==='button')return [b];if(s==='iframe')return iframe?[{src:iframe,getClientRects:()=>[1],getBoundingClientRect:()=>({height:100})}]:[];
    if(s.includes('one-time-code'))return otp?[new Input()]:[];
    if(s==='input[type="password"]')return p?[p]:[];
    return e?[e]:[];
  }},getComputedStyle:()=>({visibility:'visible'}),HTMLInputElement:Input,Event:class{},setTimeout:f=>f()};
  vm.createContext(context);vm.runInContext(fs.readFileSync('src/ui/login-assist.js','utf8'),context);
  return {run:(action='probe')=>context.loginAssist({email:'test@example.invalid',password:'fixture-secret'},action,'http://127.0.0.1:43117'),get clicks(){return clicks;},get writes(){return writes;}};
}
test('credentials only submitted to Spotify login page',async()=>{
  for(const config of [{origin:'https://evil.invalid'}, {path:'/authorize'}]) {
    const p=page({...config,email:true,password:true});await p.run('password');assert.equal(p.writes,0);assert.equal(p.clicks,0);
  }
});
test('probe never fills and password action submits once',async()=>{
  const p=page({email:true,password:true,button:'Giriş yap'});assert.equal(await p.run(),'password');assert.equal(p.writes,0);
  assert.equal(await p.run('password'),'password_submitted');assert.equal(p.writes,2);assert.equal(p.clicks,1);
});
test('email-only login and continue-account button',async()=>{
  const p=page({email:true});assert.equal(await p.run('email'),'email_submitted');assert.equal(p.clicks,1);
  const c=page({button:'Bu hesapla devam et'});assert.equal(await c.run(),'continue');assert.equal(await c.run('continue'),'continued');assert.equal(c.writes,0);
});
test('visible CAPTCHA stops before any fill or submit',async()=>{
  for(const config of [{body:'Verify you are human'},{iframe:'https://www.google.com/recaptcha/api2/bframe'}]) {
    const p=page({...config,email:true,password:true});assert.equal(await p.run('password'),'captcha_required');assert.equal(p.clicks,0);assert.equal(p.writes,0);
  }
});
test('OTP and rejected password require user without retry',async()=>{
  assert.equal(await page({otp:true,password:true}).run('password'),'verification_required');
  const p=page({body:'Incorrect password',password:true});assert.equal(await p.run('password'),'credentials_rejected');assert.equal(p.clicks,0);
});
test('return detection is scoped to the owning account origin',async()=>{
  assert.equal(await page({origin:'http://127.0.0.1:43117'}).run(),'returned');
  assert.equal(await page({origin:'http://127.0.0.1:43118'}).run(),'outside');
  assert.equal(await page({origin:'http://127.0.0.1:5588',path:'/login'}).run(),'callback');
  assert.equal(await page({origin:'http://127.0.0.1:5588',path:'/unrelated'}).run(),'outside');
});
test('Spotify Desktop authorization return is clicked without entering credentials',async()=>{
  const p=page({path:'/tr/authorize',body:'Spotify for Desktop',button:'Uygulamaya devam et'});
  assert.equal(await p.run(),'finish');assert.equal(await p.run('finish'),'continued');assert.equal(p.clicks,1);assert.equal(p.writes,0);
});
test('unknown app and generic consent are not automatically approved',async()=>{
  for(const config of [{body:'Another App',button:'Uygulamaya devam et'},{body:'Spotify for Desktop',button:'İzin ver'}]) {
    const p=page({path:'/authorize',...config});assert.equal(await p.run('continue'),'waiting');assert.equal(p.clicks,0);
  }
});
