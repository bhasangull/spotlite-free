// Executed only in the single target bound before OAuth navigation.
// Credentials never leave this expression as a result or diagnostic.
async function loginAssist(credentials, action, returnOrigin) {
  // Spotify's OAuth return briefly visits the local callback before the
  // callback page sends this same tab back to its account player.
  if (location.origin === 'http://127.0.0.1:5588' && location.pathname === '/login') return 'callback';
  if (location.origin === 'https://challenge.spotify.com') return 'captcha_required';
  if (location.origin !== 'https://accounts.spotify.com') return location.origin === returnOrigin ? 'returned' : 'outside';
  const visible = el => !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden';
  const all = selector => [...document.querySelectorAll(selector)].filter(visible);
  const body = (document.body?.innerText || '').toLowerCase();
  if (all('iframe').some(el => /(?:recaptcha|hcaptcha).*?(?:anchor|bframe|challenge)/i.test(el.src || '') && el.getBoundingClientRect().height > 60)
    || /verify you are human|confirm you.re not a robot|prove you.re human|robot olmadığınızı|insan olduğunuzu doğrula/.test(body)) return 'captcha_required';
  if (all('input[autocomplete="one-time-code"],input[name="otp"],input[name="code"]').length
    || /enter.*verification code|doğrulama kodunu gir|check your email|e-postanı kontrol et/.test(body)) return 'verification_required';
  if (/incorrect password|wrong password|yanlış şifre|şifre yanlış|incorrect username|hatalı şifre/.test(body)) return 'credentials_rejected';
  const buttons = all('button');
  // Spotify also presents a final return step on its authorization page.
  // This helper is already bound to the account's own initiated OAuth target.
  // Only the explicit return button for our existing client is eligible;
  // generic Allow/Accept/Agree consent buttons remain manual.
  if (/^\/(?:[a-z]{2}(?:-[a-z]{2})?\/)?(?:authorize|oauth2\/authorize)(?:\/|$)/i.test(location.pathname)) {
    const finish = buttons.find(el => /^(uygulamaya devam et|continue to app|continue to the app)$/i.test(el.textContent.trim()));
    if (!finish || !body.includes('spotify for desktop')) return 'waiting';
    if (action === 'finish' && !finish.disabled) { finish.click(); return 'continued'; }
    return 'finish';
  }
  // Never interact with third-party identity providers or unknown pages.
  if (!/^\/(?:[a-z]{2}(?:-[a-z]{2})?\/)?(?:login|status|continue)(?:\/|$)/i.test(location.pathname)) return 'waiting';
  const continueAccount = buttons.find(el => /^(continue as .+|.+ olarak devam et|bu hesapla devam et)$/i.test(el.textContent.trim()));
  if (continueAccount) {
    if (action === 'continue' && !continueAccount.disabled) { continueAccount.click(); return 'continued'; }
    return 'continue';
  }
  const email = all('input#login-username,input[autocomplete="username"],input[type="email"]').at(0);
  const password = all('input[type="password"]').at(0);
  const submit = buttons.find(el => /^(log in|login|sign in|continue|giriş yap|oturum aç|devam|devam et)$/i.test(el.textContent.trim()));
  if (!submit) return 'waiting';
  const kind = password ? 'password' : email ? 'email' : 'waiting';
  if (action !== kind || !credentials || kind === 'waiting') return kind;
  const set = (el, value) => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,value);el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true})); };
  if (email) set(email, credentials.email);
  if (password) set(password, credentials.password);
  await new Promise(resolve => setTimeout(resolve, 100));
  if (location.origin !== 'https://accounts.spotify.com' || !submit.isConnected || submit.disabled) return 'interaction_required';
  submit.click();
  return kind + '_submitted';
}
