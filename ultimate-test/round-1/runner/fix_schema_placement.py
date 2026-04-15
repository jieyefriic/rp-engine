#!/usr/bin/env python3
"""
Fix output_schema placement in workflow YAML files.

Problem: output_schema is placed under node.config but should be under
the corresponding template definition in the templates section.

For each generate node that has output_schema under config:
  1. Find the template name from config.template (handles both simple string
     and conditional {default, conditions} forms)
  2. Remove output_schema from the node config
  3. Add output_schema to the matching template definition in the templates section
"""

import sys
from pathlib import Path
from ruamel.yaml import YAML

FILES = [
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p01_r3_openai_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p02_r3_anthropic_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p03_r3_gemini_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p04_r3_deepseek_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p05_r3_xai_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p06_r3_qwen_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p07_r3_zhipu_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p08_r3_moonshot_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p09_r3_minimax_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim1-providers/p10_r3_huoshan_structured.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n02_generate_schema_simple.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n03_generate_schema_complex.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n04_generate_schema_nested.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n05_generate_conditional_template.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n17_loop_history.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n50_edge_conditional.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim3-nodes/n53_edge_max_iterations.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s01_blog_generator.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s02_multi_translator.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s03_content_moderation.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s04_email_drafter.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s05_seo_writer.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s13_support_router.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s14_debate.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s22_multi_step_reasoning.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s29_dead_path.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s35_large_schema.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s39_schema_chain.yaml",
    "/home/user/rp-engine/ultimate-test/round-1/workflows/dim4-scenarios/s40_conditional_loop.yaml",
]


def get_template_name(config):
    """Extract the template name from a node config.

    Handles both:
      - Simple string: template: tpl_name
      - Conditional object: template: {default: tpl_name, conditions: [...]}

    Returns the template name string (uses 'default' for conditional templates).
    """
    template = config.get("template")
    if template is None:
        return None
    if isinstance(template, str):
        return template
    # Conditional template - use the default
    if isinstance(template, dict):
        return template.get("default")
    return None


def fix_file(filepath, yaml_handler):
    """Fix output_schema placement in a single YAML file.

    Returns (modified: bool, details: list[str]) describing what was done.
    """
    path = Path(filepath)
    if not path.exists():
        return False, [f"  SKIP: file not found"]

    with open(path, "r") as f:
        data = yaml_handler.load(f)

    if data is None:
        return False, [f"  SKIP: empty file"]

    nodes = data.get("nodes", [])
    templates = data.get("templates", {})
    details = []
    modified = False

    for node in nodes:
        node_id = node.get("id", "?")
        node_type = node.get("type", "")
        config = node.get("config")
        if config is None:
            continue

        # Check if this node has output_schema under config
        if "output_schema" not in config:
            continue

        output_schema = config["output_schema"]
        template_name = get_template_name(config)

        if template_name is None:
            details.append(f"  WARNING: node '{node_id}' has output_schema but no template name found")
            continue

        if template_name not in templates:
            details.append(f"  WARNING: node '{node_id}' references template '{template_name}' not found in templates section")
            continue

        # Check if the template already has output_schema (avoid duplicates)
        tpl = templates[template_name]
        if "output_schema" in tpl:
            details.append(f"  node '{node_id}': template '{template_name}' already has output_schema, removing from node config only")
        else:
            # Add output_schema to the template
            tpl["output_schema"] = output_schema
            details.append(f"  node '{node_id}': moved output_schema -> template '{template_name}'")

        # Remove output_schema from the node config
        del config["output_schema"]
        modified = True

    if modified:
        with open(path, "w") as f:
            yaml_handler.dump(data, f)

    return modified, details


def main():
    yaml_handler = YAML()
    yaml_handler.preserve_quotes = True
    yaml_handler.width = 4096  # prevent line wrapping
    yaml_handler.best_map_representor = True

    total_modified = 0
    total_skipped = 0

    for filepath in FILES:
        name = Path(filepath).name
        modified, details = fix_file(filepath, yaml_handler)
        if modified:
            total_modified += 1
            print(f"FIXED: {name}")
        else:
            total_skipped += 1
            print(f"UNCHANGED: {name}")
        for d in details:
            print(d)

    print(f"\nSummary: {total_modified} files modified, {total_skipped} files unchanged")


if __name__ == "__main__":
    main()
