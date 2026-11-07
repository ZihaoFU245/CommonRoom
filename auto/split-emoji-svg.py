#!/usr/bin/env python3
"""Split Noto's shared SVG documents without changing glyphs or shaping tables.

Requires fonttools, brotli and lxml. Accepts input and output font paths.
Each glyph retains its original artwork and transitive SVG dependencies, while
WebKit no longer has to parse thousands of unrelated glyphs to paint one emoji.
"""

import copy
import sys

from fontTools.subset.svg import closure_element_ids, group_elements_by_id
from fontTools.ttLib import TTFont
from fontTools.ttLib.tables.S_V_G_ import SVGDocument
from lxml import etree


def split_font(source, destination):
    font = TTFont(source, recalcTimestamp=False)
    documents = []
    for document in font["SVG "].docList:
        if document.startGlyphID == document.endGlyphID:
            documents.append(document)
            continue
        root = etree.fromstring(
            document.data.encode("utf-8"),
            parser=etree.XMLParser(remove_blank_text=True, resolve_entities=False),
        )
        elements = group_elements_by_id(root)
        # Noto keeps glyph groups at the root and shared shapes/gradients in defs.
        # Reject a different source structure instead of silently altering it.
        definitions = []
        groups = []
        for child in root:
            if child.tag == "{http://www.w3.org/2000/svg}defs":
                definitions.append(child)
            elif (child.get("id") or "").startswith("glyph"):
                groups.append(child)
            else:
                raise ValueError(f"Unexpected SVG root element: {child.tag}")
        group_ids = [
            (group, set(group.xpath(".//@id")) | {group.get("id")})
            for group in groups
        ]
        definition_ids = [
            (
                container,
                [
                    (child, set(child.xpath(".//@id")) | {child.get("id")})
                    for child in container
                ],
            )
            for container in definitions
        ]
        for glyph in range(document.startGlyphID, document.endGlyphID + 1):
            required = {f"glyph{glyph}"}
            if not required.issubset(elements):
                raise ValueError(f"Missing glyph{glyph}")
            closure_element_ids(elements, required)
            result = etree.Element(root.tag, root.attrib, nsmap=root.nsmap)
            for container, children in definition_ids:
                retained = etree.Element(container.tag, container.attrib)
                for child, ids in children:
                    if ids & required:
                        retained.append(copy.deepcopy(child))
                if len(retained):
                    result.append(retained)
            for group, ids in group_ids:
                if ids & required:
                    result.append(copy.deepcopy(group))
            if not required.issubset(group_elements_by_id(result)):
                raise ValueError(f"Lost SVG dependency for glyph{glyph}")
            documents.append(
                SVGDocument(
                    etree.tostring(result).decode("utf-8"),
                    glyph,
                    glyph,
                    document.compressed,
                )
            )
    font["SVG "].docList = documents
    font.save(destination)
    print(f"Saved {len(documents)} glyph SVG documents to {destination}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(f"Usage: {sys.argv[0]} INPUT_FONT OUTPUT_FONT")
    split_font(sys.argv[1], sys.argv[2])
