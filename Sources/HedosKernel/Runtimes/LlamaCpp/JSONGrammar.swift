enum JSONGrammar {
    static let generic = """
        root ::= object
        value ::= object | array | string | number | ("true" | "false" | "null") ws
        object ::= "{" ws ( string ":" ws value ("," ws string ":" ws value)* )? "}" ws
        array ::= "[" ws ( value ("," ws value)* )? "]" ws
        string ::= "\\"" ( [^"\\\\] | "\\\\" (["\\\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F]) )* "\\"" ws
        number ::= ("-"? ([0-9] | [1-9] [0-9]*)) ("." [0-9]+)? ([eE] [-+]? [0-9]+)? ws
        ws ::= [ \\t\\n]*
        """

    static func forResponseFormat(_ value: JSONValue?) -> String? {
        guard case .object(let fields)? = value, fields["type"]?.stringValue != nil else {
            return nil
        }
        return generic
    }
}
