# NodeAffinity

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Kind** | **string** |  | 
**NodeIds** | **[]string** | The set of &#x60;node_id&#x60;s entitled to host this secret. | 
**Labels** | **map[string]string** | Required label key/value pairs. | 

## Methods

### NewNodeAffinity

`func NewNodeAffinity(kind string, nodeIds []string, labels map[string]string, ) *NodeAffinity`

NewNodeAffinity instantiates a new NodeAffinity object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeAffinityWithDefaults

`func NewNodeAffinityWithDefaults() *NodeAffinity`

NewNodeAffinityWithDefaults instantiates a new NodeAffinity object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetKind

`func (o *NodeAffinity) GetKind() string`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *NodeAffinity) GetKindOk() (*string, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *NodeAffinity) SetKind(v string)`

SetKind sets Kind field to given value.


### GetNodeIds

`func (o *NodeAffinity) GetNodeIds() []string`

GetNodeIds returns the NodeIds field if non-nil, zero value otherwise.

### GetNodeIdsOk

`func (o *NodeAffinity) GetNodeIdsOk() (*[]string, bool)`

GetNodeIdsOk returns a tuple with the NodeIds field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeIds

`func (o *NodeAffinity) SetNodeIds(v []string)`

SetNodeIds sets NodeIds field to given value.


### GetLabels

`func (o *NodeAffinity) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *NodeAffinity) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *NodeAffinity) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


