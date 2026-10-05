/** What `check` answers for a file it refuses, with the sentence the person reads. */
export class JsonRefusal {
  constructor(readonly message: string) {}
}

interface JsonImportOptions<T> {
  /**
   * What the parsed file is, or a `JsonRefusal` saying why it is not one. Gets the text as well,
   * for an import the server parses itself. Not asked about a file that is no JSON at all.
   */
  check: (parsed: unknown, text: string) => T | JsonRefusal
  /** The sentence for a file that cannot be read or is no JSON. */
  unreadable: () => string
  /** Shows a refusal, the unreadable file's included. */
  refuse: (message: string) => void
  /** Goes on with an accepted file: asks first, sends it, or keeps it for a later step. */
  take: (value: T, file: File) => unknown
}

interface JsonImport {
  /** The file the `UFileUpload` reports (`@update:model-value`); nothing for none. */
  select: (file: File | null | undefined) => Promise<void>
}

/**
 * Taking a JSON file the settings once handed out back in (WEB-09): reading, parsing and checking
 * the file a `UFileUpload` was given (RD-1110-12), then the caller's own confirmation and
 * request. Four components carried this by hand — the routing buttons kept their copy after the
 * area buttons had been generalised from it — and differed only in the format they check and
 * what follows.
 */
export function useJsonImport<T>(options: JsonImportOptions<T>): JsonImport {
  async function select(file: File | null | undefined): Promise<void> {
    if (!file) return
    let checked: T | JsonRefusal
    try {
      const text = await file.text()
      checked = options.check(JSON.parse(text) as unknown, text)
    } catch {
      checked = new JsonRefusal(options.unreadable())
    }
    if (checked instanceof JsonRefusal) return options.refuse(checked.message)
    await options.take(checked, file)
  }

  return { select }
}
