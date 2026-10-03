import { greet } from "./lib/greet";
import { suffix } from "./lib/suffix.ts";
import data from "./data.json";

// Imports its own files with and without an extension, and a JSON file.
export default (params: URLSearchParams) => ({
  greeting: greet(params.get("name") ?? "nobody") + suffix,
  count: data.count,
});
