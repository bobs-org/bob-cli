# Dependencies

- [ ] #task Blocking root [id:: root]
- [?] #task Blocked child [id:: blocked] [dependsOn:: root]
- [x] #task Done dependency [id:: done-root]
- [ ] #task Ready after done dependency [dependsOn:: done-root]
- [ ] #task Mixed dependencies [id:: mixed] [dependsOn:: root, done-root]
- [ ] #task Missing dependency is ignored [dependsOn:: missing]
- [ ] #task Self dependency [id:: self] [dependsOn:: self]
- [x] #task Duplicate id done instance [id:: duplicate]
- [ ] #task Duplicate id open instance [id:: duplicate]
- [ ] #task Duplicate id dependent [dependsOn:: duplicate]
- [-] #task Canceled dependency [id:: canceled-root]
- [ ] #task Ready after canceled dependency [dependsOn:: canceled-root]
