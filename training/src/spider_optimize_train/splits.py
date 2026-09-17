"""Train, tune, calibrate and test windows.

A pair is one trial, so its arms always land in the same window. The chronological
split cuts by whole days in order, which is the split the gates are read on: a model
is always tested on days after the ones it learned from. The domain split holds out
sites instead, to show what a model knows about a site it never saw.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

WINDOWS = ("train", "tune", "calibrate", "test")
SHARES = (0.60, 0.15, 0.10, 0.15)


@dataclass
class Split:
    train: list[dict]
    tune: list[dict]
    calibrate: list[dict]
    test: list[dict]
    description: str = ""

    def window(self, name: str) -> list[dict]:
        return getattr(self, name)

    def days(self) -> dict[str, tuple[int, int] | None]:
        out = {}
        for name in WINDOWS:
            days = [row["day"] for row in self.window(name)]
            out[name] = (min(days), max(days)) if days else None
        return out


def _pair_day(rows: list[dict]) -> dict[int, int]:
    """Each pair's day, read off its baseline arm when it has one."""
    out: dict[int, int] = {}
    for row in rows:
        if row["arm"] == "baseline" or row["pair"] not in out:
            out[row["pair"]] = row["day"]
    return out


def chronological(rows: list[dict]) -> Split:
    """60/15/10/15 of the distinct days, in order. Every row of a day goes to one
    window, and a pair goes where its baseline's day goes."""
    pair_day = _pair_day(rows)
    days = sorted(set(pair_day.values()))
    n = len(days)
    if n < len(WINDOWS):
        raise ValueError(f"{n} distinct days cannot fill {len(WINDOWS)} windows")
    counts = [max(1, int(round(share * n))) for share in SHARES[:-1]]
    while sum(counts) >= n:
        counts[int(np.argmax(counts))] -= 1
    edges = np.cumsum(counts)
    window_of_day = {}
    for at, day in enumerate(days):
        window_of_day[day] = int(np.searchsorted(edges, at, side="right"))
    buckets: list[list[dict]] = [[] for _ in WINDOWS]
    for row in rows:
        buckets[window_of_day[pair_day[row["pair"]]]].append(row)
    ranges = [
        f"{name} days {days[start]}..{days[end - 1]}"
        for name, start, end in zip(
            WINDOWS, [0, *edges], [*edges, n], strict=True
        )
    ]
    return Split(*buckets, description="chronological: " + ", ".join(ranges))


def _pair_hash(pair: int, salt: int) -> float:
    """A stable number in [0, 1) for a pair, so the inner split does not depend on row
    order."""
    x = (int(pair) * 0x9E3779B97F4A7C15 + salt * 0xBF58476D1CE4E5B9) & ((1 << 64) - 1)
    x ^= x >> 31
    x = (x * 0x94D049BB133111EB) & ((1 << 64) - 1)
    x ^= x >> 29
    return (x >> 11) / float(1 << 53)


def domain_folds(rows: list[dict], k: int = 5, salt: int = 0) -> list[Split]:
    """One split per fold: the sites with `dk % k == fold` are the test window, and the
    rest is divided by pair, 80 percent train and the other 20 percent halved into tune
    and calibrate. A pair's arms share a site, so a pair never crosses a fold."""
    out = []
    for fold in range(k):
        buckets: list[list[dict]] = [[], [], [], []]
        for row in rows:
            if int(row["dk"]) % k == fold:
                buckets[3].append(row)
                continue
            h = _pair_hash(row["pair"], salt + fold)
            buckets[0 if h < 0.8 else 1 if h < 0.9 else 2].append(row)
        out.append(Split(*buckets, description=f"domain fold {fold} of {k}"))
    return out


def recency_weight(day, latest: int, half_life: float = 30.0):
    """Half the weight every `half_life` days before the latest day."""
    age = np.maximum(np.asarray(latest, dtype=float) - np.asarray(day, dtype=float), 0.0)
    return np.power(0.5, age / half_life)
