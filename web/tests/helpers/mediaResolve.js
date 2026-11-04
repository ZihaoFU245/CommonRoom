/** A protocol-faithful resolver fixture, shared by byte-cache tests. */
export function mediaResolution(
  input,
  options,
  fields = {},
  status = 200,
  signedUrl,
) {
  const base = new URL("../../", input);
  const { items } = JSON.parse(options.body);
  return Response.json({
    items: items.map(({ kind, id, link }) => {
      const path = `${base.pathname}api/${kind}/${id}/file`;
      return status === 200
        ? {
            kind,
            id,
            status,
            lease_seconds: 900,
            info: {
              media_id: id,
              path,
              size: 6,
              content_type: "image/png",
              filename: "test.png",
              expires: null,
              expired: false,
              ...fields,
            },
            ...(link
              ? {
                  link: {
                    url: signedUrl ?? new URL(path, base).href,
                    expires_in: 60,
                  },
                }
              : {}),
          }
        : { kind, id, status, error: "Media denied" };
    }),
  });
}
