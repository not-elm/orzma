import { serveAssets } from "@ozmux/sdk/server";

serveAssets((path) =>
  path === "index.html"
    ? {
        status: 200,
        contentType: "text/html",
        body: "<!DOCTYPE html><meta charset=utf-8><h1>Hello from an ozmux extension</h1>",
      }
    : { status: 404, contentType: "text/plain", body: "not found" },
);
