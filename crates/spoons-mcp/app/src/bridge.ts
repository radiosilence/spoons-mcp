// The app's line to the host, and through it to the server: the official MCP Apps SDK.
//
// Everything the app reads or changes is a `spoons` tool call with ordinary GraphQL, exactly as the model would
// make it. What the user does here reaches the model as context (`ui/update-model-context`); what the model does
// reaches the app because it keeps watching the basket. Money moves only on the user's say-so: in the chat, or by
// two taps on the quote's Pay button, which shows the total and table; the app never pays on its own.

import {
  App,
  applyDocumentTheme,
  applyHostFonts,
  applyHostStyleVariables,
  type McpUiHostContext,
} from "@modelcontextprotocol/ext-apps";

// The spec has a View declare the display modes it can take before asking for one: the menu goes fullscreen.
export const app = new App(
  { name: "spoons", version: "1.0.0" },
  { availableDisplayModes: ["inline", "fullscreen"] },
  { autoResize: true },
);

/** The conversation's basket, so the app's own calls reach it even on an MCP session of their own. */
let session: string | null = null;

export function setSession(id: unknown) {
  if (typeof id === "string" && id) session = id;
}

export class GraphqlError extends Error {}

/** Runs GraphQL through the `spoons` tool. The app's calls ask to draw nothing. */
export async function gql<T>(query: string, variables: Record<string, unknown> = {}): Promise<T> {
  const result = await app.callServerTool({
    name: "spoons",
    arguments: {
      query,
      variables: JSON.stringify(variables),
      ui: { view: "NONE", ...(session ? { session } : {}) },
    },
  });
  // The answer is the text (compact JSON for the app's own calls); structured content only carries the view.
  const text = result.content?.find((c) => c.type === "text");
  const raw = text && "text" in text ? text.text : null;
  if (result.isError) throw new GraphqlError(raw ?? "The call failed.");
  let body: { data?: T; errors?: { message: string }[] } | undefined;
  if (raw) {
    try {
      body = JSON.parse(raw);
    } catch {
      throw new GraphqlError(raw);
    }
  } else {
    body = result.structuredContent as typeof body;
  }
  if (!body) throw new GraphqlError("The server didn’t answer.");
  // A field that failed is null and the rest still stands, as GraphQL means it; only no data at all is a failure.
  if (body.data == null) {
    throw new GraphqlError(body.errors?.map((e) => e.message).join("; ") || "No answer.");
  }
  if (body.errors?.length) console.warn("partial answer", body.errors);
  return body.data;
}

/** Tells the model what the user did here, for its next turn. It doesn't prompt a reply. */
export async function tellModel(text: string, structured?: Record<string, unknown>) {
  const can = app.getHostCapabilities()?.updateModelContext;
  if (!can) return;
  try {
    await app.updateModelContext({
      content: [{ type: "text", text }],
      ...(structured && can.structuredContent ? { structuredContent: structured } : {}),
    });
  } catch (e) {
    console.warn("couldn't update the model's context", e);
  }
}

/** Posts a message into the chat as the user, on their tap. Tried even where the host doesn't declare `message`,
 * since some handle it without saying so; a refusal just means the user asks in the chat themselves. */
export async function say(text: string): Promise<boolean> {
  try {
    const { isError } = await app.sendMessage({ role: "user", content: [{ type: "text", text }] });
    return !isError;
  } catch {
    return false;
  }
}

export function applyHost(ctx: McpUiHostContext | undefined) {
  if (!ctx) return;
  if (ctx.theme) applyDocumentTheme(ctx.theme);
  if (ctx.styles?.variables) applyHostStyleVariables(ctx.styles.variables);
  if (ctx.styles?.css?.fonts) applyHostFonts(ctx.styles.css.fonts);
  const inset = ctx.safeAreaInsets;
  const root = document.documentElement.style;
  root.setProperty("--safe-top", `${inset?.top ?? 0}px`);
  root.setProperty("--safe-right", `${inset?.right ?? 0}px`);
  root.setProperty("--safe-bottom", `${inset?.bottom ?? 0}px`);
  root.setProperty("--safe-left", `${inset?.left ?? 0}px`);
}

export function canFullscreen(): boolean {
  return app.getHostContext()?.availableDisplayModes?.includes("fullscreen") ?? false;
}

export async function setDisplayMode(mode: "inline" | "fullscreen") {
  try {
    await app.requestDisplayMode({ mode });
  } catch (e) {
    console.warn("display mode refused", e);
  }
}

export async function openLink(url: string) {
  try {
    const { isError } = await app.openLink({ url });
    if (!isError) return;
  } catch {
    /* fall through */
  }
  window.open(url, "_blank", "noopener");
}
