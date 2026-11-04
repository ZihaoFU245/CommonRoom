import { useLibraryPage } from "../../hooks/useLibraryPage.ts";
import { MediaQuota } from "../MediaQuota.tsx";
import { PickerIcon } from "../PickerIcon.tsx";

export function StickerSettings({ account = "" }: { account?: string }) {
  const { page, error, refresh } = useLibraryPage("library", {
    cacheScope: `quota:${account}`,
    retainWhileRefreshing: true,
  });
  return (
    <section class="sticker-settings" aria-label="Media storage">
      <div class="library-management-heading">
        <h4>Media storage</h4>
        <button
          type="button"
          onClick={refresh}
          aria-label="Refresh media storage"
          title="Refresh media storage"
        >
          <PickerIcon name="refresh" />
        </button>
      </div>
      {page && <MediaQuota page={page} />}
      {!page && !error && <p role="status">Loading media storage…</p>}
      {error && <p role="status">{error}</p>}
    </section>
  );
}
