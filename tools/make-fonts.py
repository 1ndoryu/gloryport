#!/usr/bin/env python3
"""Genera los TTFs estáticos que GLORYPORT incrusta (assets/fonts/).

Parte de las fuentes variables oficiales (google/fonts) y produce instancias
estáticas con la familia y el peso correctos para GDI:

  Figtree-400.ttf / Figtree-500.ttf / Figtree-600.ttf
  EBGaramond-400.ttf

Uso:
  python tools/make-fonts.py
"""

from __future__ import annotations

import argparse
from pathlib import Path

from fontTools.varLib import instancer as var_instancer
from fontTools.subset import Options, Subsetter
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "assets" / "fonts"

# Glifos necesarios para la UI: ASCII + español + signos de puntuación.
TEXT = (
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
    "0123456789áéíóúüñÁÉÍÓÚÜÑ¿¡°ºª·"
    ":.,;()[]{}<>-_–—/\\|@#$%&*+=?!'\"`´^~  "
)

SUBFAMILIES = {400: "Regular", 500: "Medium", 600: "SemiBold"}


def set_names(font: TTFont, family: str, subfamily: str, weight: int) -> None:
    """Escribe nombres coherentes para que GDI seleccione por familia + peso."""
    name = font["name"]
    ps_name = f"{family.replace(' ', '')}-{subfamily}"
    full = f"{family} {subfamily}"
    unique = f"{full};GLORYPORT;2026;{weight}"
    for name_id, value in {
        1: family,          # Familia
        2: subfamily,       # Subfamilia
        3: unique,          # Identificador único
        4: full,            # Nombre completo
        6: ps_name,         # Nombre PostScript
        16: family,         # Familia tipográfica
        17: subfamily,      # Subfamilia tipográfica
    }.items():
        name.setName(value, name_id, 3, 1, 0x409)
        name.setName(value, name_id, 1, 0, 0)
    os2 = font["OS/2"]
    os2.usWeightClass = weight
    fs = os2.fsSelection
    fs &= ~0b0100_0001  # limpia bit 0 (italic) y bit 6 (regular) para no-regulares
    if weight == 400:
        fs |= 0b0100_0000  # bit 6: regular
    os2.fsSelection = fs


def make_static(variable: Path, out: Path, family: str, weight: int) -> None:
    font = TTFont(variable)
    var_instancer.instantiateVariableFont(font, {"wght": weight})
    set_names(font, family, SUBFAMILIES[weight], weight)
    options = Options()
    options.flavor = None
    options.desubroutinize = True
    options.hinting = False
    subsetter = Subsetter(options=options)
    subsetter.populate(text=TEXT)
    subsetter.subset(font)
    font.save(out)
    print(f"OK  {out.name} ({out.stat().st_size} bytes)")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--figtree", default=str(FONTS / "Figtree-Variable.ttf"))
    parser.add_argument("--ebgaramond", default=str(FONTS / "EBGaramond-Variable.ttf"))
    args = parser.parse_args()

    make_static(Path(args.figtree), FONTS / "Figtree-400.ttf", "Figtree", 400)
    make_static(Path(args.figtree), FONTS / "Figtree-500.ttf", "Figtree", 500)
    make_static(Path(args.figtree), FONTS / "Figtree-600.ttf", "Figtree", 600)
    make_static(Path(args.ebgaramond), FONTS / "EBGaramond-400.ttf", "EB Garamond", 400)


if __name__ == "__main__":
    main()
