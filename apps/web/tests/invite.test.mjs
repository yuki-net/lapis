import { strict as assert } from "node:assert";
import { test } from "node:test";
import { hostWebSocketUrl, parseInviteFragment, selectClientId } from "../src/invite.ts";

test("同じ公開入口の WebSocket を使い、招待情報を送らない", () => {
  assert.equal(
    hostWebSocketUrl("http://100.64.0.1:4747/#/invite?token=secret"),
    "ws://100.64.0.1:4747/remote",
  );
  assert.equal(
    hostWebSocketUrl("https://lapis.example/host/workspace?backend=webgl#/invite?token=secret"),
    "wss://lapis.example/remote",
  );
  assert.throws(() => hostWebSocketUrl("file:///index.html"));
});

test("招待による再登録だけ新しい端末 ID を使う", () => {
  assert.equal(selectClientId("token", "old-device", () => "new-device"), "new-device");
  assert.equal(selectClientId(null, "old-device", () => "new-device"), "old-device");
  assert.equal(selectClientId(null, null, () => "new-device"), "new-device");
});

test("招待は token 一つだけの新形式を受け付ける", () => {
  const token = "ab".repeat(32);
  assert.equal(parseInviteFragment(`#/invite?token=${token}`).valid, true);
  assert.equal(parseInviteFragment(`#/invite?token=${token}&workspace=old`).valid, false);
  assert.equal(parseInviteFragment(`#/invite?token=${token}&token=${token}`).valid, false);
  assert.equal(parseInviteFragment("#/invite?token=short").valid, false);
  assert.equal(parseInviteFragment("#/t/123"), null);
});
