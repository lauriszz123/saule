-- `:SauleRun` / `:SauleRunFile` — the equivalent of the IntelliJ plugin's two
-- run configurations: `saule run` for a project, `saule run <file>` for a
-- single script.
--
-- The command runs in a terminal split so program output, colours and exit
-- status land where the plugin's Run tool window puts them. Arguments after
-- the command name are passed to the script, separated by `--` exactly as the
-- plugin does — without it the CLI reads the first program argument as the run
-- target and rejects the rest.

local toolchain = require("saule.toolchain")

local M = {}

--- Locate `saule`, reporting the same hint the other editors do when missing.
local function locate_cli()
  local located = toolchain.locate("saule", {
    env = "SAULE_PATH",
    cmd = vim.g.saule_cli_path,
    dir = vim.g.saule_toolchain_dir,
    start = toolchain.start_dir(0),
  })
  if not located.found then
    vim.notify(
      "saule: could not find the 'saule' executable. Build it with "
        .. "`cargo build --release`, set `vim.g.saule_cli_path`, or add "
        .. "'saule' to your PATH.",
      vim.log.levels.ERROR
    )
    return nil
  end
  return located
end

local function run(args, program_args)
  local located = locate_cli()
  if not located then
    return
  end

  local cmd = { located.command, "run" }
  vim.list_extend(cmd, args)
  if program_args and program_args ~= "" then
    -- `--` separates the run target from the script's own argv.
    table.insert(cmd, "--")
    vim.list_extend(cmd, vim.split(program_args, "%s+", { trimempty = true }))
  end

  vim.cmd("botright new")
  -- `jobstart({ term = true })` is Neovim 0.11+; on older versions the key is
  -- silently ignored and the output would go nowhere, so use `termopen` there.
  if vim.fn.has("nvim-0.11") == 1 then
    vim.fn.jobstart(cmd, { cwd = located.working_dir, term = true })
  else
    vim.fn.termopen(cmd, { cwd = located.working_dir })
  end
  vim.cmd("startinsert")
end

--- `saule run` — project mode.
function M.project(program_args)
  run({}, program_args)
end

--- `saule run <file>` — single-file mode. Writes the buffer first, so what
--- runs is what is on screen.
function M.file(program_args)
  local path = vim.api.nvim_buf_get_name(0)
  if path == nil or path == "" then
    vim.notify("saule: the current buffer has no file name.", vim.log.levels.ERROR)
    return
  end
  if vim.bo.modified then
    vim.cmd("write")
  end
  run({ path }, program_args)
end

return M
