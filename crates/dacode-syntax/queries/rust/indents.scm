; Nodes whose inner lines sit one level in from the line they start on, and the closing tokens
; that pull a line back out. Helix's capture names, written for this grammar; see
; `structure.rs` for how they are read.
[
  (block)
  (declaration_list)
  (field_declaration_list)
  (ordered_field_declaration_list)
  (enum_variant_list)
  (match_block)
  (match_arm)
  (arguments)
  (parameters)
  (closure_parameters)
  (type_parameters)
  (type_arguments)
  (tuple_expression)
  (tuple_type)
  (tuple_pattern)
  (slice_pattern)
  (struct_pattern)
  (array_expression)
  (field_initializer_list)
  (use_list)
  (token_tree)
  (where_clause)
  (let_declaration)
] @indent

[ "}" ")" "]" ] @outdent
