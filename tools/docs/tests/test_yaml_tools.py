from bob_docs.yaml_tools import InertPythonName, dump_yaml, load_yaml_text


def test_python_name_tag_round_trips_without_importing() -> None:
    source = """\
format: !!python/name:pymdownx.superfences.fence_code_format ''
future: !!python/name:bob_docs.fences.fence_format ''
"""
    loaded = load_yaml_text(source)
    assert loaded["format"] == InertPythonName(
        "tag:yaml.org,2002:python/name:pymdownx.superfences.fence_code_format", ""
    )
    assert loaded["future"] == InertPythonName(
        "tag:yaml.org,2002:python/name:bob_docs.fences.fence_format", ""
    )

    dumped = dump_yaml(loaded)
    reloaded = load_yaml_text(dumped)
    assert reloaded == loaded


def test_duplicate_yaml_keys_are_rejected() -> None:
    try:
        load_yaml_text("one: 1\none: 2\n")
    except Exception as error:
        assert "duplicate key" in str(error)
    else:
        raise AssertionError("duplicate YAML key unexpectedly accepted")
