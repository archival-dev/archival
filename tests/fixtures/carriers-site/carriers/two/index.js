export default async (params, body, objects, site) => ({
  artist: objects.artist[0].name,
  objects: Object.keys(objects).includes("SITE_URL"),
  url: site.url,
  site: Object.keys(site),
});
