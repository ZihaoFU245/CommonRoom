import type { LibraryPage } from "../api/library.ts";
function storageSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const unit = Math.min(3, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / 1024 ** unit).toLocaleString(undefined, { maximumFractionDigits: 1 })} ${["B", "KiB", "MiB", "GiB"][unit]}`;
}

export function MediaQuota({ page }: { page: LibraryPage }) {
  return (
    <div
      class="library-quota"
      title={`${page.usage.toLocaleString()} / ${page.quota.toLocaleString()} bytes`}
    >
      <div>
        <span>Media storage</span>
        <strong>
          {storageSize(page.usage)} / {storageSize(page.quota)}
        </strong>
      </div>
      <div
        class="library-quota-track"
        data-used={page.usage > 0 ? "true" : "false"}
        role="progressbar"
        aria-valuenow={Math.min(page.usage, page.quota)}
        aria-valuemin={0}
        aria-valuemax={Math.max(1, page.quota)}
        aria-label="Media storage used"
      >
        <span
          class="library-quota-fill"
          style={{
            transform: `scaleX(${Math.min(1, Math.max(0, page.usage / Math.max(1, page.quota)))})`,
          }}
        />
      </div>
    </div>
  );
}
