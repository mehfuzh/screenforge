// Cloudflare R2 (S3 API) for share links: presigned uploads straight from the app, so video
// bytes never pass through the edge functions, plus HEAD and DELETE from here.
//
// Secrets: R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY (an R2 API token with Object
// Read & Write on the bucket), R2_BUCKET (default screenforge-shares) and SHARE_MEDIA_URL, the
// bucket's public custom domain (default https://media.screenforge.studio).
import { AwsClient } from "https://esm.sh/aws4fetch@1.0.20";

const BUCKET = Deno.env.get("R2_BUCKET") ?? "screenforge-shares";
export const MEDIA_URL = (Deno.env.get("SHARE_MEDIA_URL") ?? "https://media.screenforge.studio").replace(/\/+$/, "");

export function configured(): boolean {
  return !!(Deno.env.get("R2_ACCOUNT_ID") && Deno.env.get("R2_ACCESS_KEY_ID") && Deno.env.get("R2_SECRET_ACCESS_KEY"));
}

function client(): AwsClient {
  return new AwsClient({ accessKeyId: Deno.env.get("R2_ACCESS_KEY_ID") ?? "", secretAccessKey: Deno.env.get("R2_SECRET_ACCESS_KEY") ?? "", service: "s3", region: "auto" });
}

const objectUrl = (key: string) => `https://${Deno.env.get("R2_ACCOUNT_ID")}.r2.cloudflarestorage.com/${BUCKET}/${key}`;

/** A URL the app PUTs `bytes` bytes of `contentType` to within `secs`. Content-Type and
 *  Content-Length are signed: an upload of another type or size is refused by R2. */
export async function presignPut(key: string, contentType: string, bytes: number, secs = 6 * 3600): Promise<string> {
  const url = new URL(objectUrl(key));
  url.searchParams.set("X-Amz-Expires", String(secs));
  const req = new Request(url, { method: "PUT", headers: { "Content-Type": contentType, "Content-Length": String(bytes) } });
  const signed = await client().sign(req, { aws: { signQuery: true, allHeaders: true } });
  return signed.url;
}

/** The stored object's size, or null when there is none. */
export async function objectSize(key: string): Promise<number | null> {
  const r = await client().fetch(objectUrl(key), { method: "HEAD" });
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`R2 HEAD ${r.status}`);
  return Number(r.headers.get("content-length") ?? "0");
}

export async function deleteObject(key: string): Promise<void> {
  const r = await client().fetch(objectUrl(key), { method: "DELETE" });
  if (!r.ok && r.status !== 404) throw new Error(`R2 DELETE ${r.status}`);
}

/** Object keys of a link. */
export const videoKey = (id: string) => `v/${id}/video.mp4`;
export const posterKey = (id: string) => `v/${id}/poster.jpg`;
