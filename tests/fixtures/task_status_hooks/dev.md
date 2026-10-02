# Development

- [ ] #task Promote me [dependsOn:: dev__dep-one, dev__done-dep] ^promote
  - ![[#^dep-one]]
  - ![[#^done-dep]]
  - ![[#^ref]]
  - [[#^plain]]
  ```md
  - ![[#^fenced-dep]]
  ```
- [ ] #task Same-file dependency [id:: dev__dep-one] [dependsOn:: Projects__Alpha__dep-two] ^dep-one
  - ![[Projects/Alpha#^dep-two]]
- [x] #task Completed dependency stays done [id:: dev__done-dep] ^done-dep
- [ ] #task Plain link is not a dependency ^plain
- [ ] #task Fenced transclusion is not a dependency ^fenced-dep
- [*] #task Stale dependency clears ^stale-child
- [*] #task Already next ^already
- [*] #task Clear me ^orphan
  - ![[#^stale-child]]
- [ ] #task Closed reference stays todo ^closed
- [x] #task Done stays done ^done
- [-] #task Cancelled stays cancelled ^cancelled
- [!] #task Unknown stays unknown ^unknown
- [*] Not a Tasks task ^not-a-task

Reference block only. ^ref
