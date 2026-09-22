#!/usr/bin/env node
// Asks archival to build and deploy this run's commit, then waits for the site
// to go live, so the run's status is the deploy's status.

import { appendFileSync } from "node:fs";

const POLL_MS = 5_000;
// A deploy in progress survives a few failed polls: the api may be mid-deploy.
const MAX_POLL_FAILURES = 12;

const env = (name) => {
  const value = process.env[name];
  if (!value) {
    console.log(`::error::${name} is not set`);
    process.exit(1);
  }
  return value;
};

const host = env("ARCHIVAL_API_HOST").replace(/\/+$/, "");
const token = env("ARCHIVAL_GITHUB_TOKEN");
const [owner, repo] = env("GITHUB_REPOSITORY").split("/");
const runId = env("GITHUB_RUN_ID");
const runAttempt = env("GITHUB_RUN_ATTEMPT");
const sha = env("GITHUB_SHA");
const deadline =
  Date.now() + Number(process.env.ARCHIVAL_TIMEOUT_MINUTES || 30) * 60_000;

const buildsUrl = `${host}/builds/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`;
const headers = {
  authorization: `Bearer ${token}`,
  "content-type": "application/json",
  "user-agent": "archival-action",
};

// Workflow commands end at a newline, so a multi-line message is escaped.
const command = (name, message, title) =>
  console.log(
    `::${name}${title ? ` title=${title}` : ""}::${String(message)
      .replace(/%/g, "%25")
      .replace(/\r/g, "%0D")
      .replace(/\n/g, "%0A")}`,
  );

const summary = (markdown) => {
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${markdown}\n`);
  }
};

const errorOf = async (res) => {
  const text = await res.text().catch(() => "");
  try {
    return JSON.parse(text).error ?? text;
  } catch {
    return text || `HTTP ${res.status}`;
  }
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const start = async () => {
  for (let attempt = 1; ; attempt++) {
    let res;
    try {
      res = await fetch(buildsUrl, {
        method: "POST",
        headers,
        body: JSON.stringify({
          runId: Number(runId),
          runAttempt: Number(runAttempt),
          sha,
        }),
      });
    } catch (e) {
      res = { ok: false, status: 0, text: async () => `${e}` };
    }
    if (res.ok) {
      return;
    }
    if ((res.status === 0 || res.status >= 500) && attempt < 5) {
      console.log(`Could not reach archival (${res.status}), retrying...`);
      await sleep(POLL_MS * attempt);
      continue;
    }
    command("error", await errorOf(res), "Deploy not started");
    process.exit(1);
  }
};

const wait = async () => {
  let cursor = 0;
  let failures = 0;
  let lastDescription = "";
  while (Date.now() < deadline) {
    let status;
    try {
      const res = await fetch(`${buildsUrl}/${runId}?cursor=${cursor}`, {
        headers,
      });
      if (!res.ok) {
        const message = await errorOf(res);
        if (res.status < 500) {
          command("error", message, "Deploy status unavailable");
          process.exit(1);
        }
        throw new Error(message);
      }
      status = await res.json();
      failures = 0;
    } catch (e) {
      if (++failures >= MAX_POLL_FAILURES) {
        command("error", `${e}`, "Lost contact with archival");
        process.exit(1);
      }
      await sleep(POLL_MS);
      continue;
    }
    if (status.logs.length) {
      console.log("::group::Build output");
      for (const line of status.logs) {
        console.log(line);
      }
      console.log("::endgroup::");
      cursor = status.cursor;
    }
    if (status.description && status.description !== lastDescription) {
      console.log(status.description);
      lastDescription = status.description;
    }
    switch (status.state) {
      case "success":
        command("notice", status.siteUrl ?? "Deployed", "Site is live");
        summary(
          `### Deployed${status.siteUrl ? `\n\n${status.siteUrl}` : ""}`,
        );
        return 0;
      case "failure":
        command("error", status.description, "Deploy failed");
        summary(`### Deploy failed\n\n\`\`\`\n${status.description}\n\`\`\``);
        return 1;
      case "cancelled":
        command("notice", status.description, "Not deployed");
        return 0;
      case "waiting_for_domain":
        command(
          "warning",
          "This site has no domain yet, so there is nowhere to deploy it.",
          "Not deployed",
        );
        return 0;
    }
    await sleep(POLL_MS);
  }
  command(
    "error",
    `The site did not go live within ${process.env.ARCHIVAL_TIMEOUT_MINUTES || 30} minutes.`,
    "Deploy timed out",
  );
  return 1;
};

console.log(`Deploying ${owner}/${repo}@${sha.slice(0, 7)} with archival...`);
await start();
process.exit(await wait());
