import brandMark from "../assets/commonroom.svg?no-inline";

export function Brand() {
  return (
    <div class="brand">
      <svg
        class="brand-mark"
        viewBox="0 0 1024 1024"
        width={36}
        height={36}
        aria-hidden="true"
      >
        <use href={`${brandMark}#brand-mark`} />
      </svg>
      <span>CommonRoom</span>
    </div>
  );
}
