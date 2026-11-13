# Tools, Marketplace, and canvas brush

The [OPM Registry](https://github.com/ximocm/opm-registry) is a separate repository
of reusable tool definitions and native pipelines. Its
[documentation](https://github.com/ximocm/opm-registry#readme) describes the
available tools, program requirements, standalone command-line use, and example
workflows. Tools are maintained there independently of the OPM application.

## Install or import a definition

Open **Pipeline > Marketplace…**, or **Marketplace** in a Design window. Search
the catalog and choose **Add to OPM** on a tool or pipeline. The default public
source is `ximocm/opm-registry`, branch `dev`; the source fields also accept
another public GitHub repository and a branch, tag, or commit. Refresh reads
all definitions from one resolved commit, so a moving branch cannot mix versions
within the displayed catalog.

- Tools appear in **Installed tools** and the Design window's **Tools** palette.
- Pipelines appear in **Project > Open pipeline** and **New work**.
- **Import tool JSON…** installs a definition from a local file without network
  access. **Project > Import pipeline** also accepts native `.pipeline` files,
  alongside YAML/JSON interchange files.
- **Update** replaces an installed definition only after an explicit choice.
  Existing steps and working folders retain their copies.

Adding a tool installs its definition. The program itself, such as FastQC or
Cutadapt, must be available in the environment used for execution. Browsing,
importing, and installing never run downloaded commands or install programs.

## Paint steps

1. Create or open a pipeline in a **Design** window.
2. Choose an installed tool from the **Tools** palette, or choose **Use brush**
   in Marketplace's **Installed tools** tab. From another window, using a tool
   opens a Design window.
3. Choose the optional arguments to include and the number of files for list
   inputs. For example, FastQC with two reads gets two separate file inputs.
4. Choose **Use brush**, then click empty canvas to place a step. The brush
   stays active for repeated placements. Each step receives a unique ID and
   its own output locations. Pan and zoom are respected when placing a step.
5. Press **Esc** on the canvas, or choose **Selection tool**, to stop painting.
   Existing cards remain selectable; painting is available only in Design.
6. Edit each step's defaults and output paths and connect dependencies or
   previous outputs as usual. Save the pipeline to retain the steps and layout.

Argument groups and input counts are expanded into normal pipeline declarations
when a step is placed. To change them for subsequent placements, select the tool
again; existing steps remain editable through the standard step editor.

## Local storage and reproducibility

Tools live in the per-user application data directory, in `opm/tools/`, or the
directory named by `OPM_TOOLS_DIR`. Each installed tool occupies one JSON file.
The Marketplace shows the resolved location. The pipeline library continues to
use `OPM_PIPELINES_DIR` or the default `opm/pipelines/` directory.

The installed tool records its registry repository, exact commit and path. A
painted step includes its tool version and provenance and embeds its command,
inputs, parameters and outputs. Saved pipelines and work folders can therefore
run without the registry or the installed tool file; external programs are
still required. Program arguments retain OPM's placeholder quoting and visible
validation rules.

Catalog requests run in a background thread, use HTTPS, and have timeouts and
response-size limits. If GitHub is unavailable or rate-limits a request, the
dialog reports the error and installed tools remain usable offline. No GitHub
credentials are needed for a public catalog.
