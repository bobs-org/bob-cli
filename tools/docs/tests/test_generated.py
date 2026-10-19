from pathlib import Path

import pytest

from bob_docs.errors import DocsError
from bob_docs.generated import replace_generated_region


def test_region_replacement_preserves_surrounding_bytes() -> None:
    source = "prefix\n<!-- BEGIN GENERATED: one -->\nold\n<!-- END GENERATED: one -->\nsuffix\n"
    result = replace_generated_region(source, "one", "new\nsecond")
    assert result == "prefix\n<!-- BEGIN GENERATED: one -->\nnew\nsecond\n<!-- END GENERATED: one -->\nsuffix\n"


@pytest.mark.parametrize(
    "source",
    [
        "<!-- BEGIN GENERATED: one -->\n<!-- END GENERATED: one -->\n<!-- END GENERATED: one -->\n",
        "<!-- END GENERATED: one -->\n<!-- BEGIN GENERATED: one -->\n",
        "nothing here\n",
    ],
)
def test_region_replacement_rejects_malformed_markers(source: str) -> None:
    with pytest.raises(DocsError):
        replace_generated_region(source, "one", "body")


def test_yaml_region_markers_allow_indentation() -> None:
    source = "nav:\n  # BEGIN GENERATED: nav\n  old: page.md\n  # END GENERATED: nav\n"
    result = replace_generated_region(source, "nav", "  new: page.md", yaml=True)
    assert result == "nav:\n  # BEGIN GENERATED: nav\n  new: page.md\n  # END GENERATED: nav\n"
