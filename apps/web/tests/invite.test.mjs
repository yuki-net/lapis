import { strict as assert } from "node:assert";
import { test } from "node:test";
import { hostIdentity, parseInviteFragment, selectClientId } from "../src/invite.ts";

test("Host の識別子は同じ公開入口の Origin だけを持ち、招待情報を送らない", () => {
  assert.equal(
    hostIdentity("http://100.64.0.1:4747/#/invite?token=secret"),
    "http://100.64.0.1:4747/",
  );
  assert.equal(
    hostIdentity("https://lapis.example/host/workspace?backend=webgl#/invite?token=secret"),
    "https://lapis.example/",
  );
  assert.throws(() => hostIdentity("file:///index.html"));
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
