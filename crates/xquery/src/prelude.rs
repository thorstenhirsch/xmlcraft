//! Helper functions the compiler calls, written in XQuery (compiled with every query, in an internal namespace): the keys of
//! `order by` and `group by`, one stable sort pass, grouping, window boundaries, and the checks of computed constructors.

/// The prelude's source.
pub(crate) const PRELUDE: &str = r#"
module namespace xqi = "http://xmlcraft.org/ns/xquery-internal";

declare function xqi:raise($code as xs:string, $message as xs:string) as item()* {
  error(QName('http://www.w3.org/2005/xqt-errors', concat('err:', $code)), $message)
};

(: An order or group key: atomized, at most one value, untyped values compared as strings (XQuery 3.1 §3.12.7, §3.12.8). :)
declare function xqi:key($k as item()*) as xs:anyAtomicType? {
  let $a := data($k)
  return
    if (count($a) gt 1) then xqi:raise('XPTY0004', 'an order or group key has more than one item')
    else if ($a instance of xs:untypedAtomic) then string($a)
    else $a
};

(: One stable pass of `order by` over the tuples by member $i. Empty keys sort first (`empty least`) or last; descending is
   reverse, sort, reverse, which keeps ties in order. :)
declare function xqi:sort-pass($t as array(*)*, $i as xs:integer, $desc as xs:boolean, $eg as xs:boolean,
                               $coll as xs:string?) as array(*)* {
  let $key := function($x as array(*)) as xs:anyAtomicType* {
    let $k := $x($i)
    return if (empty($k)) then (if ($eg) then 1 else 0) else ((if ($eg) then 0 else 1), $k)
  }
  return if ($desc) then reverse(sort(reverse($t), $coll, $key)) else sort($t, $coll, $key)
};

declare function xqi:key-eq($x as xs:anyAtomicType?, $y as xs:anyAtomicType?, $coll as xs:string?) as xs:boolean {
  if (empty($x)) then empty($y)
  else if (empty($y)) then false()
  else if (empty($coll) or $coll eq '') then deep-equal($x, $y)
  else deep-equal($x, $y, $coll)
};

declare function xqi:same-key($a as array(*), $b as array(*), $n as xs:integer, $m as xs:integer,
                              $colls as xs:string*) as xs:boolean {
  every $i in 1 to $m satisfies xqi:key-eq($a($n + $i), $b($n + $i), $colls[$i])
};

(: `group by`: tuples [$v1 … $vn, key1 … keym] become one per group, [$v1 … $vn joined over the group, keys], in the order
   the groups first appear. :)
declare function xqi:group($tuples as array(*)*, $n as xs:integer, $m as xs:integer, $colls as xs:string*) as array(*)* {
  let $firsts := fold-left($tuples, (), function($acc, $t) {
    if (some $a in $acc satisfies xqi:same-key($a, $t, $n, $m, $colls)) then $acc else ($acc, $t)
  })
  for $f in $firsts
  let $members := $tuples[xqi:same-key(., $f, $n, $m, $colls)]
  return array:join((
    for $v in 1 to $n return [ for $t in $members return $t($v) ],
    array:subarray($f, $n + 1, $m)
  ))
};

declare function xqi:switch-key($v as item()*) as xs:anyAtomicType? {
  let $a := data($v)
  return if (count($a) gt 1) then xqi:raise('XPTY0004', 'a switch operand or case has more than one item') else $a
};

declare function xqi:switch-match($k as xs:anyAtomicType?, $v as item()*) as xs:boolean {
  deep-equal($k, xqi:switch-key($v))
};

(: Tumbling windows as [start, end] positions (XQuery 3.1 §3.12.4.1). :)
declare function xqi:tumbling($n as xs:integer, $start as function(*), $end as function(*)?, $only as xs:boolean) as array(*)* {
  if (empty($end)) then
    let $st := fold-left(1 to $n, [0, ()], function($st, $i) {
      if ($start($i)) then [$i, ($st(2), if ($st(1) ne 0) then [$st(1), $i - 1] else ())] else $st
    })
    return ($st(2), if ($st(1) ne 0) then [$st(1), $n] else ())
  else
    let $st := fold-left(1 to $n, [0, ()], function($st, $i) {
      if ($st(1) eq 0) then
        if ($start($i)) then (if ($end($i, $i)) then [0, ($st(2), [$i, $i])] else [$i, $st(2)]) else $st
      else if ($end($st(1), $i)) then [0, ($st(2), [$st(1), $i])]
      else $st
    })
    return ($st(2), if ($st(1) ne 0 and not($only)) then [$st(1), $n] else ())
};

(: Sliding windows as [start, end] positions (XQuery 3.1 §3.12.4.2). :)
declare function xqi:sliding($n as xs:integer, $start as function(*), $end as function(*)?, $only as xs:boolean) as array(*)* {
  for $s in 1 to $n
  where $start($s)
  let $e := (for $j in $s to $n where $end($s, $j) return $j)[1]
  return if (exists($e)) then [$s, $e] else if ($only) then () else [$s, $n]
};

(: The name of a computed element or attribute from its value (XQuery 3.1 §3.9.3.1, §3.9.3.2). :)
declare function xqi:qname($v as item()*, $ns as map(*), $default as xs:string, $element as xs:boolean) as xs:QName {
  let $a := data($v)
  let $q :=
    if (count($a) ne 1) then xqi:raise('XPTY0004', 'a computed name must be one value')
    else if ($a instance of xs:QName) then $a
    else if ($a instance of xs:string or $a instance of xs:untypedAtomic) then
      let $s := normalize-space(string($a))
      let $parts := tokenize($s, ':')
      return
        if (count($parts) eq 2 and $parts[1] castable as xs:NCName and $parts[2] castable as xs:NCName) then
          if (map:contains($ns, $parts[1])) then QName($ns($parts[1]), $s)
          else xqi:raise('XQDY0074', concat('the prefix ', $parts[1], ' is not declared'))
        else if (count($parts) eq 1 and $s castable as xs:NCName) then QName($default, $s)
        else xqi:raise('XQDY0074', concat('''', $s, ''' is not a name'))
    else xqi:raise('XPTY0004', 'a computed name must be a QName or a string')
  return
    if (not($element) and local-name-from-QName($q) eq 'xmlns' and namespace-uri-from-QName($q) eq '')
    then xqi:raise('XQDY0044', 'an attribute cannot be named xmlns')
    else $q
};

declare function xqi:ns-prefix($v as item()*) as xs:string {
  let $s := normalize-space(string-join(data($v), ''))
  return if ($s eq '' or $s castable as xs:NCName) then $s else xqi:raise('XQDY0074', concat('''', $s, ''' is not a prefix'))
};

declare function xqi:ns-uri($p as xs:string, $u as xs:string) as xs:string {
  if ($p eq 'xmlns' or $u eq 'http://www.w3.org/2000/xmlns/' or ($p eq 'xml') ne ($u eq 'http://www.w3.org/XML/1998/namespace')
      or ($u eq '' and $p ne ''))
  then xqi:raise('XQDY0101', concat('the prefix ''', $p, ''' cannot be bound to ''', $u, ''''))
  else $u
};

declare function xqi:comment-text($s as xs:string) as xs:string {
  if (contains($s, '--') or ends-with($s, '-')) then xqi:raise('XQDY0072', 'a comment cannot contain ''--'' or end with ''-''')
  else $s
};

declare function xqi:pi-target($v as item()*) as xs:string {
  let $s := normalize-space(string-join(data($v), ''))
  return
    if (not($s castable as xs:NCName)) then xqi:raise('XQDY0041', concat('''', $s, ''' is not a processing-instruction target'))
    else if (lower-case($s) eq 'xml') then xqi:raise('XQDY0064', 'a processing instruction cannot be named xml')
    else $s
};

declare function xqi:pi-content($s as xs:string) as xs:string {
  if (contains($s, '?>')) then xqi:raise('XQDY0026', 'a processing instruction cannot contain ''?>''')
  else replace($s, '^\s+', '')
};
"#;
