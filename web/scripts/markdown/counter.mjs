export const counterSource = String.raw`<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Simple JS Counter</title>
<style>
body {font-family:Arial,sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0;background:#f4f4f9;}
.counter-container {text-align:center;background:white;padding:2rem;border-radius:10px;box-shadow:0 4px 6px #0002;}
h1 {font-size:4rem;margin:0 0 1rem;color:#333;}
button {font-size:1rem;padding:.5rem 1rem;margin:0 .5rem;cursor:pointer;border:0;border-radius:5px;background:#007bff;color:white;}
button#reset {background:#6c757d;}
</style></head><body>

    <div class="counter-container">
        <h1 id="counter-value">0</h1>
        <button id="decrease">Decrease</button>
        <button id="reset">Reset</button>
        <button id="increase">Increase</button>
    </div>

    <script>
        const counterValue = document.getElementById('counter-value');
        let count = 0;
        document.getElementById('increase').addEventListener('click', () => { counterValue.textContent = ++count; });
        document.getElementById('decrease').addEventListener('click', () => { counterValue.textContent = --count; });
        document.getElementById('reset').addEventListener('click', () => { count = 0; counterValue.textContent = count; });
    </script>
</body></html>`;
export const isolationSource = String.raw`<!doctype html><html><body><h2>Frame isolation checks</h2><pre id="result">Checking…</pre><script>
(async () => {
  const result = {};
  try { result.parentDOM = !!parent.document.body; } catch { result.parentDOM = 'blocked'; }
  try { result.storage = localStorage.length; } catch { result.storage = 'blocked'; }
  try { result.cookie = document.cookie; } catch { result.cookie = 'blocked'; }
  try { await fetch('/api/me'); result.network = 'allowed'; } catch { result.network = 'blocked'; }
  result.origin = self.origin;
  document.getElementById('result').textContent = JSON.stringify(result, null, 2);
})();
</script></body></html>`;
