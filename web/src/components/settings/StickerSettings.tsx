import { useLibraryPage } from "../../hooks/useLibraryPage.ts";
import { MediaQuota } from "../MediaQuota.tsx";

export function StickerSettings({ account = "" }: { account?: string }) {
  const { page, error } = useLibraryPage("library", {
    cacheScope: account,
    retainWhileRefreshing: true,
  });
  return (
    <section class="sticker-settings" aria-label="Media storage">
      <h4>Media storage</h4>
      {page && <MediaQuota page={page} />}
      {!page && !error && <p role="status">Loading media storage…</p>}
      {error && <p role="status">{error}</p>}
    </section>
  );
}
