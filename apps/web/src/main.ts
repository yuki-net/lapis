import "./style.css";

const root = document.querySelector<HTMLElement>("#app");

if (root === null) {
  throw new Error("Lapis Web root was not found");
}

root.innerHTML = `
  <section class="card" aria-labelledby="title">
    <p class="eyebrow">LAPIS WEB</p>
    <h1 id="title">Client foundation is ready.</h1>
    <p>Protocol and backend connection will be added next.</p>
    <span class="status"><i aria-hidden="true"></i>Vite + TypeScript</span>
  </section>
`;
