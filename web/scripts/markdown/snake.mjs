// A complete inline-script game used as untrusted message content in the fixture.
export const snakeSource = String.raw`<h3>Snake game</h3>
<p>Use arrow keys to steer. Eat the apple to grow.</p>
<p>Score: <span id="snake-score">0</span></p>
<pre id="snake-board">⬜⬜⬜⬜⬜⬜⬜⬜
⬜⬜⬜⬜⬜⬜⬜⬜
⬜⬜⬜⬜⬜🍎⬜⬜
⬜⬜🟩🟩🐍⬜⬜⬜
⬜⬜⬜⬜⬜⬜⬜⬜
⬜⬜⬜⬜⬜⬜⬜⬜</pre>
<button onclick="startSnake()">Start game</button>
<style>#snake-board { background: #eff6e9; padding: 12px; }</style>
<script>
document.documentElement.dataset.snakeExecuted = "yes";
let snake = [[4,3],[3,3],[2,3]], dir = [1,0], apple = [5,2], timer;
function drawSnake() {
  document.getElementById("snake-board").textContent = Array.from({length:6}, (_,y) => Array.from({length:8}, (_,x) => snake.some(p => p[0]===x && p[1]===y) ? (snake[0][0]===x && snake[0][1]===y ? "🐍" : "🟩") : apple[0]===x && apple[1]===y ? "🍎" : "⬜").join("")).join("\n");
}
function startSnake() {
  clearInterval(timer);
  timer = setInterval(() => {
    const head = [snake[0][0]+dir[0], snake[0][1]+dir[1]];
    if (head[0]<0 || head[0]>7 || head[1]<0 || head[1]>5 || snake.some(p => p[0]===head[0] && p[1]===head[1])) { clearInterval(timer); return; }
    snake.unshift(head);
    if (head[0]===apple[0] && head[1]===apple[1]) { apple=[(apple[0]+3)%8,(apple[1]+2)%6]; document.getElementById("snake-score").textContent=String(snake.length-3); } else snake.pop();
    drawSnake();
  }, 200);
}
document.addEventListener("keydown", e => {
  const next = {ArrowUp:[0,-1],ArrowDown:[0,1],ArrowLeft:[-1,0],ArrowRight:[1,0]}[e.key];
  if (next && (next[0]!==-dir[0] || next[1]!==-dir[1])) { dir=next; e.preventDefault(); }
});
</script>`;
