; A Python block has no closing token, so the statements that own one are also `@extend`: the
; cursor at the very end of one is still inside it while its line sits deeper than the head.
[
  (function_definition)
  (class_definition)
  (if_statement)
  (elif_clause)
  (else_clause)
  (for_statement)
  (while_statement)
  (try_statement)
  (except_clause)
  (finally_clause)
  (with_statement)
  (match_statement)
  (case_clause)
] @indent @extend

[
  (argument_list)
  (parameters)
  (lambda_parameters)
  (list)
  (dictionary)
  (set)
  (tuple)
  (parenthesized_expression)
  (list_comprehension)
  (dictionary_comprehension)
  (set_comprehension)
  (generator_expression)
  (subscript)
] @indent

[ ")" "]" "}" ] @outdent
