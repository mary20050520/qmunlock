"""从 QQ音乐 Mac 的 plist 读取 API 认证凭据（uin + authst）。

凭据在（旧版沙盒 / 新版 11.x 非沙盒，两处都可能存在，取最近修改的）：
  ~/Library/Containers/com.tencent.QQMusicMac/Data/Library/Preferences/com.tencent.QQMusicMac.plist
  ~/Library/Preferences/com.tencent.QQMusicMac.plist
  → 键 AutoLoginUserInfo（NSKeyedArchiver 二进制）→ 内含 UserInfo 对象：
      nUserId / strUserAccount : QQ UIN
      strAuthst                : API 认证令牌（调 GetEVkey 必需）
      loginType                : 1=QQ, 3=微信

注意：authst 会过期（约 22h），QQ音乐 会定期用 strRefreshKey 刷新。
若 API 报 405/空，多半是 authst 过期，重新登录或等 app 刷新即可。
"""
import os
import plistlib
from plistlib import UID

CANDIDATE_PLISTS = [
    os.path.expanduser(
        "~/Library/Containers/com.tencent.QQMusicMac/"
        "Data/Library/Preferences/com.tencent.QQMusicMac.plist"
    ),
    os.path.expanduser("~/Library/Preferences/com.tencent.QQMusicMac.plist"),
]


def find_plist(candidates=CANDIDATE_PLISTS):
    """返回存在的候选中最近修改的那个；都不存在时返回第一个（用于报错提示）。"""
    existing = [p for p in candidates if os.path.exists(p)]
    if not existing:
        return candidates[0]
    return max(existing, key=os.path.getmtime)


def _resolve(objs, x):
    """把 NSKeyedArchiver 的 UID 引用解析成实际值。"""
    if isinstance(x, UID):
        return objs[x.data]
    return x


def get_credentials(plist_path=None):
    """返回 {'uin','authst','loginType','openid','refresh_key'}。

    找不到凭据时抛 CredentialsError。
    """
    plist_path = plist_path or find_plist()
    if not os.path.exists(plist_path):
        raise CredentialsError(f"plist 不存在: {plist_path}（QQ音乐 没装/没登录过？）")
    with open(plist_path, 'rb') as f:
        top = plistlib.load(f)
    if 'AutoLoginUserInfo' not in top:
        raise CredentialsError("plist 里没有 AutoLoginUserInfo（未登录）")
    inner = plistlib.loads(top['AutoLoginUserInfo'])
    objs = inner['$objects']
    for o in objs:
        if isinstance(o, dict) and 'strAuthst' in o:
            return {
                'uin': str(_resolve(objs, o.get('nUserId', ''))),
                'authst': _resolve(objs, o.get('strAuthst', '')),
                'loginType': str(_resolve(objs, o.get('loginType', '3'))),
                'openid': _resolve(objs, o.get('strOpenId', '')),
                'refresh_key': _resolve(objs, o.get('strRefreshKey', '')),
            }
    raise CredentialsError("AutoLoginUserInfo 里没找到 UserInfo(strAuthst)")


class CredentialsError(Exception):
    pass
