"""Ed25519 (RFC 8032) の署名と検証。

配布 manifest の署名専用であり、通信の暗号には使わない。
実装は RFC 8032 6 章の参考実装に基づき、7.1 のテストベクタで一致を確認する。
秘密鍵は 32 byte の seed として扱う。
"""

from __future__ import annotations

import hashlib

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)
IDENTITY = (0, 1, 1, 0)


class Ed25519Error(ValueError):
    """鍵または署名の長さ・形式が不正。"""


def _sha512(data: bytes) -> bytes:
    return hashlib.sha512(data).digest()


def _point_add(p, q):
    a = (p[1] - p[0]) * (q[1] - q[0]) % P
    b = (p[1] + p[0]) * (q[1] + q[0]) % P
    c = 2 * p[3] * q[3] * D % P
    d = 2 * p[2] * q[2] % P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _point_mul(scalar: int, point):
    result = IDENTITY
    while scalar > 0:
        if scalar & 1:
            result = _point_add(result, point)
        point = _point_add(point, point)
        scalar >>= 1
    return result


def _point_equal(p, q) -> bool:
    return (
        (p[0] * q[2] - q[0] * p[2]) % P == 0
        and (p[1] * q[2] - q[1] * p[2]) % P == 0
    )


def _recover_x(y: int, sign: int) -> int | None:
    if y >= P:
        return None
    x2 = (y * y - 1) * pow(D * y * y + 1, P - 2, P) % P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


def _compress(point) -> bytes:
    inverse = pow(point[2], P - 2, P)
    x = point[0] * inverse % P
    y = point[1] * inverse % P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _decompress(data: bytes):
    if len(data) != 32:
        return None
    packed = int.from_bytes(data, "little")
    sign = packed >> 255
    y = packed & ((1 << 255) - 1)
    x = _recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, x * y % P)


def _base_point():
    y = 4 * pow(5, P - 2, P) % P
    x = _recover_x(y, 0)
    if x is None:
        raise Ed25519Error("Ed25519 の base point を復元できない")
    return (x, y, 1, x * y % P)


BASE = _base_point()


def _expand(seed: bytes):
    if len(seed) != 32:
        raise Ed25519Error("秘密鍵は 32 byte の seed")
    digest = _sha512(seed)
    scalar = int.from_bytes(digest[:32], "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    return scalar, digest[32:]


def public_key(seed: bytes) -> bytes:
    """seed から 32 byte の公開鍵を返す。"""
    scalar, _ = _expand(seed)
    return _compress(_point_mul(scalar, BASE))


def sign(seed: bytes, message: bytes) -> bytes:
    """seed で message に署名し、64 byte の署名を返す。"""
    scalar, prefix = _expand(seed)
    public = _compress(_point_mul(scalar, BASE))
    r = int.from_bytes(_sha512(prefix + message), "little") % L
    encoded_r = _compress(_point_mul(r, BASE))
    k = int.from_bytes(_sha512(encoded_r + public + message), "little") % L
    s = (r + k * scalar) % L
    return encoded_r + s.to_bytes(32, "little")


def verify(public: bytes, message: bytes, signature: bytes) -> bool:
    """署名を検証する。形式不正も False を返す。"""
    if len(public) != 32 or len(signature) != 64:
        return False
    point_a = _decompress(public)
    encoded_r = signature[:32]
    point_r = _decompress(encoded_r)
    if point_a is None or point_r is None:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= L:
        return False
    k = int.from_bytes(_sha512(encoded_r + public + message), "little") % L
    return _point_equal(_point_mul(s, BASE), _point_add(point_r, _point_mul(k, point_a)))
