export default async (params, body, objects, site) => {
  await site.sql.exec("CREATE TABLE IF NOT EXISTS visits (at TEXT)");
  if (body) {
    await site.sql.exec("INSERT INTO visits VALUES (?)", new Date());
  }
  const [{ count }] = await site.sql.exec(
    "SELECT count(*) AS count FROM visits",
  );
  return { count };
};
