"""A synthetic corpus with known answers.

Nothing here was fetched. The generator invents about 300 sites with random 64-bit
keys, sends pairs of arms against them on 60 days, and draws each outcome from
probabilities written into the manifest's `planted` block. A test reads a planted
number from that block and checks the pipeline recovers it, so no test copies a
number the pipeline printed.

Arms of one pair usually share one uniform draw for success and one noise draw for
time and cost, the way two fetches of the same page a minute apart share the page, so
a candidate with a higher success chance rarely loses a pair its baseline won. One
pair in ten draws the candidate's success on its own, which is where those rare
losses come from.

Three scenarios share the generator. `default` is the corpus every fixture was made
from. `reversal` plants a strong residential effect on blocked sites that holds
through the train, tune and calibrate days and flips inside the chronological test
window only, so a model trained on the evidence applies the edit and the evaluation
has to reject the artifact. `stable` is the same plant with no flip, the control a
passing run must clear with edits applied rather than by abstaining. `SCENARIOS`
below says what each one changes.
"""

from __future__ import annotations

import copy
import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import labels, splits
from . import schema as sch
from .dataset import Manifest

DAYS = 60
DOMAINS = 320
REPEAT_SHARE = 0.05
UNCOUPLED_SHARE = 0.10

PLANTED = {
    "days": DAYS,
    "domains": DOMAINS,
    "wait": {
        "wire": "wait_for",
        "wait_ms": 5000,
        "ext": "markup",
        "mem": "cold",
        "success_from": 0.55,
        "success_to": 0.90,
        "add_millis": 4000,
        "credit_factor": 1.5,
    },
    "residential": {
        "wire": "proxy",
        "status": "blocked",
        "success_from": 0.15,
        "success_to": 0.85,
        "credit_factor": 3.0,
        "flipped_last_days": 15,
        "success_when_flipped": 0.15,
    },
    "stylesheets": {
        "wire": "block_stylesheets",
        "millis_factor": 0.7,
        "credit_factor": 0.9,
        "breaks_ext": "other",
        "jaccard_below": 0.5,
    },
    "blacklist": {
        "wire": "network_blacklist",
        "third_party_share_min": 3,
        "bytes_factor": 0.6,
        "first_party_breaks": True,
    },
    "browser": {
        "wire": "request",
        "status": "empty",
        "success_from": 0.2,
        "success_to": 0.9,
        "credit_factor": 4.0,
    },
    "repeats": {"share": REPEAT_SHARE, "beta": [20, 1]},
    "uncoupled_share": UNCOUPLED_SHARE,
}

# How often each edit is tried. The five planted edits carry most of the corpus; the
# other four keep enough rows to pass the validator's floor.
EDIT_WEIGHTS = {
    "wait_for": 0.18,
    "proxy": 0.14,
    "request": 0.14,
    "block_stylesheets": 0.16,
    "network_blacklist": 0.18,
    "disable_intercept": 0.04,
    "full_resources": 0.04,
    "block_ads": 0.04,
    "block_analytics": 0.03,
}

# Keys a synthetic caller pins. None of them is learnable.
PINNABLE = (5, 7, 10, 11, 79)

# How a site's labels are drawn, in the order the draws happen.
STATUS_WEIGHTS = {"ok": 0.72, "blocked": 0.16, "empty": 0.12}
EXT_WEIGHTS = {"markup": 0.62, "other": 0.14, "json": 0.08, "none": 0.08, "pdf": 0.04,
               "feed": 0.04}
NEED_WEIGHTS = {"markdown": 0.7, "text": 0.08, "html": 0.06, "links": 0.06, "metadata": 0.05,
                "fields": 0.05}
NEED_WEIGHTS_OTHER = {"markdown": 0.7, "text": 0.2, "html": 0.1}
TARGET_SHARE = 0.5


@dataclass(frozen=True)
class Scenario:
    """What one scenario changes. `default` is the generator as every fixture knows it;
    the others are the regression fixtures the module docstring describes."""

    name: str
    planted: dict
    edit_weights: dict = field(default_factory=lambda: EDIT_WEIGHTS)
    domains: int = DOMAINS
    status_weights: dict = field(default_factory=lambda: STATUS_WEIGHTS)
    ext_weights: dict = field(default_factory=lambda: EXT_WEIGHTS)
    need_weights: dict = field(default_factory=lambda: NEED_WEIGHTS)
    need_weights_other: dict = field(default_factory=lambda: NEED_WEIGHTS_OTHER)
    target_share: float = TARGET_SHARE  # chance a planted edit is tried where its effect is

    @property
    def uncoupled_share(self) -> float:
        return float(self.planted["uncoupled_share"])


# The first day of the chronological test window when every one of the DAYS days has a
# pair, which a corpus of a few hundred pairs or more always does.
FLIP_DAY = sum(splits.window_counts(DAYS))


def _scenario_planted(flip: bool) -> dict:
    """The plant the two regression scenarios share. The residential and wait effects are
    strong enough to be cheaper per correct result than keep, the way the sweep needs
    them, and the credit and time factors are small enough that a policy applying both
    on half the pairs can pass the window-wide cost and latency checks. `flip` turns the
    residential uplift into a loss from the first test day on; nothing before that day
    differs between the two."""
    p = copy.deepcopy(PLANTED)
    p["wait"].update({"success_from": 0.55, "success_to": 0.75, "add_millis": 200,
                      "credit_factor": 1.1})
    p["residential"].update({
        "success_from": 0.25,
        "success_to": 0.90,
        "credit_factor": 1.5,
        "millis_factor": 1.1,
        "flip_day": FLIP_DAY if flip else None,
        "flipped_last_days": DAYS - FLIP_DAY if flip else 0,
        "success_when_flipped": 0.05 if flip else 0.90,
    })
    # Arms of one pair share their success draw almost always, so the risk the sweep
    # bounds comes from the plant and not from luck.
    p["uncoupled_share"] = 0.005
    p["domains"] = 800
    return p


_SCENARIO_MIX = {
    # More blocked sites, and most tries of the two planted edits where their effect is,
    # so each floor sees 200 covered rows on the calibrate window and the top
    # need/ext/mem cell of each edit is seen on 50 sites in train.
    "edit_weights": {
        "wait_for": 0.16,
        "proxy": 0.40,
        "request": 0.08,
        "block_stylesheets": 0.12,
        "network_blacklist": 0.12,
        "disable_intercept": 0.03,
        "full_resources": 0.03,
        "block_ads": 0.03,
        "block_analytics": 0.03,
    },
    "domains": 800,
    "status_weights": {"ok": 0.53, "blocked": 0.35, "empty": 0.12},
    "ext_weights": {"markup": 0.80, "other": 0.10, "json": 0.05, "none": 0.05},
    "need_weights": {"markdown": 0.85, "text": 0.05, "html": 0.05, "links": 0.05},
    "target_share": 0.8,
}

SCENARIOS = {
    "default": Scenario("default", PLANTED),
    "reversal": Scenario("reversal", {"scenario": "reversal", **_scenario_planted(True)},
                         **_SCENARIO_MIX),
    "stable": Scenario("stable", {"scenario": "stable", **_scenario_planted(False)},
                       **_SCENARIO_MIX),
}


@dataclass
class Site:
    dk: int
    ext: str
    need: str
    tld: int
    status: str  # "ok", "blocked" or "empty": how a plain fetch of it goes
    mem: str
    depth: int
    third_party_hosts: int  # -1 when nothing was observed
    millis: float
    credits: float
    bytes: float

    @property
    def base_success(self) -> float:
        return base_success(self, PLANTED)


def base_success(site: Site, planted: dict) -> float:
    """How a plain fetch of the site goes under a plant."""
    if site.status == "blocked":
        return planted["residential"]["success_from"]
    if site.status == "empty":
        return planted["browser"]["success_from"]
    wait = planted["wait"]
    if site.ext == wait["ext"] and site.mem == wait["mem"]:
        return wait["success_from"]
    return 0.93


def _pick(rng: np.random.Generator, options: dict):
    names = list(options)
    weights = np.array([options[n] for n in names], dtype=float)
    return names[int(rng.choice(len(names), p=weights / weights.sum()))]


def make_sites(rng: np.random.Generator, count: int = DOMAINS,
               scenario: Scenario | None = None) -> list[Site]:
    scenario = scenario or SCENARIOS["default"]
    sites = []
    for _ in range(count):
        status = _pick(rng, scenario.status_weights)
        ext = _pick(rng, scenario.ext_weights)
        if ext == "other":
            need = _pick(rng, scenario.need_weights_other)
        else:
            need = _pick(rng, scenario.need_weights)
        if status == "ok":
            mem = _pick(rng, {"cold": 0.75, "thin": 0.12, "warm": 0.13})
        else:
            mem = "warm"
        sites.append(
            Site(
                dk=int(rng.integers(0, np.iinfo(np.uint64).max, dtype=np.uint64)),
                ext=ext,
                need=need,
                tld=int(rng.integers(0, 6)),
                status=status,
                mem=mem,
                depth=int(rng.integers(0, 7)),
                third_party_hosts=int(rng.integers(0, 14)) if ext == "markup" else -1,
                millis=float(rng.lognormal(np.log(1500), 0.4)),
                credits=float(rng.lognormal(np.log(1.2), 0.3)),
                bytes=float(rng.lognormal(np.log(80_000), 0.5)),
            )
        )
    return sites


def base_features(site: Site, pinned: bool) -> list[float]:
    schema = sch.load()
    x = [0.0] * sch.BASE_DIM
    x[sch.BASE_PATH_DEPTH + site.depth] = 1.0
    x[sch.BASE_EXTENSION + sch.EXT_LABELS.index(site.ext)] = 1.0
    x[sch.BASE_TLD + site.tld] = 1.0
    x[sch.BASE_NEED + schema.needs.index(site.need)] = 1.0
    if pinned:
        x[sch.BASE_PINS] = 1.0
    x[sch.BASE_MEM_COUNT + sch.MEMORY_LABELS.index(site.mem)] = 1.0
    if site.mem != "cold":
        x[sch.BASE_MEM_STATUS + schema.status.index(site.status)] = 1.0
    x[sch.BASE_BIAS] = 1.0
    return x


def routed_mode(site: Site) -> str:
    return "smart" if site.status == "blocked" else "http"


@dataclass(frozen=True)
class Edit:
    """One edit as a row describes it."""

    wire: str
    op: int  # 0 set, 1 append
    bucket: int
    ident: dict | None = None

    @property
    def key(self) -> int:
        return sch.load().key_index(self.wire)

    def descriptor(self) -> dict:
        return {"key": self.key, "op": self.op, "bucket": self.bucket, "ident": self.ident}


def standard_edit(wire: str, rng: np.random.Generator | None = None) -> Edit:
    """The edit the generator tries for a key. A blacklist append draws its identifier."""
    if wire == "wait_for":
        return Edit(wire, 0, sch.WAIT_BUCKETS.index(PLANTED["wait"]["wait_ms"]))
    if wire == "proxy":
        return Edit(wire, 0, sch.PROXIES.index("residential"))
    if wire == "request":
        return Edit(wire, 0, sch.MODES.index("browser"))
    if wire in ("block_stylesheets", "disable_intercept", "full_resources"):
        return Edit(wire, 0, 1)
    if wire in ("block_ads", "block_analytics"):
        return Edit(wire, 0, 0)
    if wire == "network_blacklist":
        rng = rng or np.random.default_rng(0)
        third = bool(rng.random() < 0.7)
        return blacklist_edit(third, int(rng.integers(0, 5)), int(rng.integers(0, 4)))
    raise ValueError(wire)


def blacklist_edit(third: bool, share: int, rank: int = 0) -> Edit:
    ident = {"class": sch.EXT_LABELS.index("asset"), "share": share, "count": 1, "third": third}
    return Edit("network_blacklist", 1, rank, ident)


def edit_features(site: Site, edit: Edit | None) -> list[float]:
    """The edit feature vector for a site and an edit, following
    `spider_optimize::features::featurize_edit` block by block."""
    schema = sch.load()
    b = schema.block
    x = [0.0] * schema.edit_dim

    x[b("config") + sch.MODES.index(routed_mode(site))] = 1.0
    x[b("config") + 3 + sch.PROXIES.index("isp")] = 1.0
    x[b("config") + 5 + 0] = 1.0  # no wait
    # disable intercept off, full resources off, block ads on, analytics on,
    # stylesheets off.
    for at, on in enumerate((0, 0, 1, 1, 0)):
        x[b("config") + 9 + at * 2 + on] = 1.0

    ident = None
    if edit is not None:
        slot = [k["index"] for k in schema.learnable].index(edit.key)
        x[b("edit_key") + slot] = 1.0
        x[b("edit_op") + edit.op] = 1.0
        width = schema.blocks["value_bucket"][1]
        x[b("value_bucket") + min(edit.bucket, width - 1)] = 1.0
        x[b("pair") + 4] = 1.0  # a single edit is not one of the four pairs
        ident = edit.ident

    if site.need == "fields":
        x[b("need_bits") + 1] = 1.0
    elif site.need in ("links", "metadata"):
        x[b("need_bits") + 2] = 1.0
    elif site.need == "screenshot":
        x[b("need_bits") + 3] = 1.0
    else:
        x[b("need_bits")] = 1.0

    if site.third_party_hosts >= 0:
        # None, 1 to 2, 3 to 5, 6 to 10, 11 or more.
        bucket = sum(site.third_party_hosts > edge for edge in (0, 2, 5, 10))
        x[b("obs_hosts") + bucket] = 1.0
    if ident is not None:
        x[b("obs_share") + min(ident["share"], 4)] = 1.0
        x[b("obs_class") + ident["class"]] = 1.0
        x[b("obs_party") + int(ident["third"])] = 1.0

    if site.third_party_hosts < 0:
        x[b("missing")] = 1.0
    if ident is None:
        x[b("missing") + 1] = 1.0
    if site.mem == "cold":
        x[b("missing") + 2] = 1.0
    x[b("bias")] = 1.0
    return x


def site_of(row: dict) -> Site:
    """The parts of a site a row's features show, enough to featurize another edit on
    it. Time, cost and size are not recovered."""
    schema = sch.load()
    base = row["base"]
    status = "ok"
    if row["mem"] != "cold":
        at = base[sch.BASE_MEM_STATUS : sch.BASE_MEM_STATUS + len(schema.status)].index(1)
        status = schema.status[at]
    hosts_block = row["edit_feats"][schema.block("obs_hosts") : schema.block("obs_share")]
    hosts = -1
    if 1 in hosts_block:
        hosts = (0, 1, 3, 6, 11)[hosts_block.index(1)]
    return Site(
        dk=row["dk"], ext=row["ext"], need=row["need"], tld=row["tld"], status=status,
        mem=row["mem"], depth=base[: 7].index(1), third_party_hosts=hosts,
        millis=0.0, credits=0.0, bytes=0.0,
    )


def with_edit(row: dict, edit: Edit | None) -> dict:
    """A copy of a row's inputs as if it had carried `edit`."""
    out = dict(row)
    out["edit"] = None if edit is None else edit.descriptor()
    out["edit_feats"] = edit_features(site_of(row), edit)
    return out


def effect(site: Site, edit: Edit | None, day: int, planted: dict | None = None) -> dict:
    """What an edit does to a site on a day, as factors on the baseline arm."""
    p = PLANTED if planted is None else planted
    out = {
        "success": base_success(site, p),
        "millis_factor": 1.0,
        "add_millis": 0.0,
        "credit_factor": 1.0,
        "bytes_factor": 1.0,
        "breaks": False,
    }
    if edit is None:
        return out
    if edit.wire == "wait_for":
        out["add_millis"] = p["wait"]["add_millis"]
        out["credit_factor"] = p["wait"]["credit_factor"]
        if site.status == "ok" and site.ext == p["wait"]["ext"] and site.mem == p["wait"]["mem"]:
            out["success"] = p["wait"]["success_to"]
    elif edit.wire == "proxy":
        out["credit_factor"] = p["residential"]["credit_factor"]
        out["millis_factor"] = p["residential"].get("millis_factor", 1.3)
        if site.status == p["residential"]["status"]:
            flipped = day >= p["days"] - p["residential"]["flipped_last_days"]
            key = "success_when_flipped" if flipped else "success_to"
            out["success"] = p["residential"][key]
    elif edit.wire == "request":
        out["credit_factor"] = p["browser"]["credit_factor"]
        out["millis_factor"] = 2.5
        if site.status == p["browser"]["status"]:
            out["success"] = p["browser"]["success_to"]
    elif edit.wire == "block_stylesheets":
        out["millis_factor"] = p["stylesheets"]["millis_factor"]
        out["credit_factor"] = p["stylesheets"]["credit_factor"]
        out["breaks"] = site.ext == p["stylesheets"]["breaks_ext"]
    elif edit.wire == "network_blacklist":
        ident = edit.ident or {}
        if not ident.get("third"):
            out["bytes_factor"] = p["blacklist"]["bytes_factor"]
            out["credit_factor"] = 0.8
            out["breaks"] = p["blacklist"]["first_party_breaks"]
        elif ident.get("share", 0) >= p["blacklist"]["third_party_share_min"]:
            out["bytes_factor"] = p["blacklist"]["bytes_factor"]
            out["millis_factor"] = 0.9
            out["credit_factor"] = 0.8
        else:
            out["bytes_factor"] = 0.95
            out["credit_factor"] = 0.97
    elif edit.wire == "full_resources":
        out["millis_factor"] = 1.3
        out["credit_factor"] = 1.2
    elif edit.wire in ("block_ads", "block_analytics"):
        out["millis_factor"] = 1.05
        out["credit_factor"] = 1.02
    elif edit.wire == "disable_intercept":
        out["millis_factor"] = 1.02
    return out


def _fail_status(site: Site) -> str:
    return {"blocked": "blocked", "empty": "empty"}.get(site.status, "server_error")


def _row(
    *,
    pair: int,
    arm: str,
    day: int,
    site: Site,
    edit: Edit | None,
    pins: tuple[int, int],
    success: bool,
    millis: float,
    bytes_: float,
    credits: float,
    jaccard: float | None,
    ratio: float | None,
    fields_present: int,
    fields_ok: bool | None,
) -> dict:
    schema = sch.load()
    pinned, pinned_hi = pins
    requested = {"links": 10, "metadata": 10, "fields": 3}.get(site.need, 0)
    row = {
        "v": 1,
        "schema_v": schema.schema_version,
        "feat_v": sch.BASE_FEATURE_VERSION,
        "edit_feat_v": schema.edit_feature_version,
        "pair": pair,
        "arm": arm,
        "day": day,
        "dk": site.dk,
        "need": site.need,
        "ext": site.ext,
        "tld": site.tld,
        "mem": site.mem,
        "routed": {
            "mode": routed_mode(site),
            "proxy": "isp",
            "wait_ms": 0,
            "start_rung": 0,
            "source": "heuristic",
            "confidence": 0.7,
        },
        "edit": None if edit is None else edit.descriptor(),
        "pinned": pinned,
        "pinned_hi": pinned_hi,
        "success": bool(success),
        "status": "ok" if success else _fail_status(site),
        "millis": int(round(millis)),
        "bytes": int(round(bytes_)) if success else 0,
        "credits": round(float(credits), 5),
        "attempts": 1,
        "multiplier": 1.0,
        "fields_requested": requested if success else 0,
        "fields_present": fields_present if success else 0,
        "content_ok": None,
        "fields_ok": fields_ok,
        "shingle_jaccard": jaccard,
        "byte_ratio": ratio,
        "base": [_compact(v) for v in base_features(site, pinned != 0 or pinned_hi != 0)],
        "edit_feats": [_compact(v) for v in edit_features(site, edit)],
    }
    row["content_ok"] = labels.content_ok(row) if arm != "baseline" else None
    return row


def _compact(value: float):
    return int(value) if float(value).is_integer() else float(value)


def draw_pair(rng: np.random.Generator, pair: int, day: int, site: Site, edit: Edit | None,
              repeat: bool, scenario: Scenario | None = None) -> list[dict]:
    scenario = scenario or SCENARIOS["default"]
    u = rng.random()
    noise = rng.lognormal(0.0, 0.25)
    pins = (0, 0)
    if rng.random() < 0.1:
        keys = rng.choice(PINNABLE, size=int(rng.integers(1, 4)), replace=False)
        low = sum(1 << int(k) for k in keys if k < 32)
        high = sum(1 << (int(k) - 32) for k in keys if k >= 32)
        pins = (low, high)

    base_fx = effect(site, None, day, scenario.planted)
    cand_fx = effect(site, edit, day, scenario.planted)
    b_success = u < base_fx["success"]
    # Most of the time the second fetch sees the page the first did; now and then it
    # draws its own luck, which is where a regression on a better edit comes from.
    c_u = rng.random() if rng.random() < scenario.uncoupled_share else u
    c_success = c_u < cand_fx["success"]

    b_millis = site.millis * noise
    b_bytes = site.bytes * rng.lognormal(0.0, 0.05)
    b_credits = site.credits * rng.lognormal(0.0, 0.05)
    c_noise = rng.lognormal(0.0, 0.05)
    c_millis = (site.millis * noise * cand_fx["millis_factor"] + cand_fx["add_millis"]) * c_noise
    c_bytes = b_bytes * cand_fx["bytes_factor"] * rng.lognormal(0.0, 0.02)
    c_credits = b_credits * cand_fx["credit_factor"] * c_noise

    jaccard = ratio = None
    fields_present, fields_ok = {"links": 10, "metadata": 10, "fields": 3}.get(site.need, 0), None
    if site.need == "fields":
        fields_ok = True
    if b_success and c_success:
        if repeat:
            a, b = PLANTED["repeats"]["beta"]
            jaccard = float(rng.beta(a, b))
        elif cand_fx["breaks"]:
            jaccard = float(rng.uniform(0.1, PLANTED["stylesheets"]["jaccard_below"] - 0.01))
        else:
            jaccard = float(rng.beta(60, 1))
        ratio = min(b_bytes, c_bytes) / max(b_bytes, c_bytes)
        if cand_fx["breaks"] and site.need in ("links", "metadata"):
            fields_present = 5
        if cand_fx["breaks"] and site.need == "fields":
            fields_ok = False
    baseline = _row(
        pair=pair, arm="baseline", day=day, site=site, edit=None, pins=pins,
        success=b_success, millis=b_millis, bytes_=b_bytes, credits=b_credits,
        jaccard=None, ratio=None,
        fields_present={"links": 10, "metadata": 10, "fields": 3}.get(site.need, 0),
        fields_ok=True if site.need == "fields" else None,
    )
    if repeat:
        c_millis, c_bytes, c_credits = b_millis * c_noise, b_bytes, b_credits * c_noise
        c_success = b_success
    candidate = _row(
        pair=pair, arm="candidate", day=day, site=site, edit=None if repeat else edit,
        pins=pins, success=c_success, millis=c_millis, bytes_=c_bytes, credits=c_credits,
        jaccard=None if jaccard is None else round(jaccard, 5),
        ratio=None if ratio is None else round(ratio, 5),
        fields_present=fields_present, fields_ok=fields_ok,
    )
    return [baseline, candidate]


def _site_for(rng: np.random.Generator, sites: list[Site], wire: str,
              scenario: Scenario) -> Site:
    """Most tries of a planted edit go where its effect is, so the corpus holds enough
    of each; the rest go anywhere."""
    p = scenario.planted
    wait = p["wait"]
    wanted = {
        "wait_for": lambda s: s.status == "ok" and s.ext == wait["ext"] and s.mem == wait["mem"],
        "proxy": lambda s: s.status == p["residential"]["status"],
        "request": lambda s: s.status == p["browser"]["status"],
        "block_stylesheets": lambda s: s.ext == p["stylesheets"]["breaks_ext"],
    }.get(wire)
    if wanted is not None and rng.random() < scenario.target_share:
        pool = [s for s in sites if wanted(s)]
        if pool:
            return pool[int(rng.integers(0, len(pool)))]
    return sites[int(rng.integers(0, len(sites)))]


def scenario_named(name: str) -> Scenario:
    if name not in SCENARIOS:
        raise ValueError(f"unknown scenario {name!r}; one of {', '.join(SCENARIOS)}")
    return SCENARIOS[name]


def generate_rows(seed: int, pairs: int = 4000, scenario: str = "default") -> list[dict]:
    sc = scenario_named(scenario)
    rng = np.random.default_rng(seed)
    sites = make_sites(rng, sc.domains, sc)
    first_pair = int(rng.integers(1, 1 << 40))
    rows = []
    for n in range(pairs):
        day = int(rng.integers(0, DAYS))
        repeat = rng.random() < REPEAT_SHARE
        if repeat:
            site = sites[int(rng.integers(0, len(sites)))]
            edit = None
        else:
            wire = _pick(rng, sc.edit_weights)
            site = _site_for(rng, sites, wire, sc)
            edit = standard_edit(wire, rng)
        rows.extend(draw_pair(rng, first_pair + n, day, site, edit, repeat, sc))
    return rows


def generate(out: Path, seed: int, pairs: int = 4000, scenario: str = "default") -> Manifest:
    sc = scenario_named(scenario)
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    rows = generate_rows(seed, pairs, scenario)
    schema = sch.load()
    with open(out / "rows.jsonl", "w") as handle:
        for row in rows:
            handle.write(json.dumps(row, separators=(",", ":")) + "\n")
    manifest = Manifest(
        schema_version=schema.schema_version,
        feature_version=sch.BASE_FEATURE_VERSION,
        edit_feature_version=schema.edit_feature_version,
        edit_dim=schema.edit_dim,
        rows=len(rows),
        pairs=pairs,
        day_min=min(r["day"] for r in rows),
        day_max=max(r["day"] for r in rows),
        collector_rev="synthetic",
        client_version="synthetic",
        service_revision="none",
        credits_spent=0.0,
        tau=labels.DEFAULT_TAU,
        salt_id=f"synth-{seed}" if scenario == "default" else f"synth-{seed}-{scenario}",
        synthetic=True,
        planted=sc.planted,
    )
    manifest.write(out)
    return manifest
